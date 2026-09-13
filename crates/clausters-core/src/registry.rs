//! The finite-resource registry: the one id allocator model, shared by the
//! server and every client.
//!
//! Node ids, buses and buffers are finite server resources fixed at boot; an
//! id allocator is the **registry** of that resource's usage, not a name
//! generator. The invariants (project rule, see `docs/decisions.md`):
//!
//! 1. every released resource becomes allocatable again;
//! 2. no monotonically increasing counter;
//! 3. no operation may lose track of a resource — a failed release reports
//!    instead of silently dropping, exhaustion is an explicit `None`, never a
//!    wrap.
//!
//! The registry is a dense occupancy map over `[base, base + capacity)` with a
//! next-fit scan hint, so a run of `width` contiguous ids (a multichannel bus)
//! allocates and coalesces with no free-list bookkeeping. It is **passive**:
//! callers feed it events (an `/node_end` arrival, an engine rejection) — it
//! never calls out, which keeps it identical across the FFI bindings and
//! wasm-compatible.
//!
//! The one sanctioned exception is [`Registry::unbounded`], for NRT/score
//! rendering: an offline render has no real-time bound on concurrent
//! resources and no live `/node_end` stream to recycle from, so its id space is
//! deliberately inexhaustible (and `release` degrades to live-count
//! accounting).

/// The **smallest** private-bus range GraphDef instances reserve at the top of
/// the audio space, whatever the space is: enough for a handful of nested
/// graphs on the smallest server anybody would boot.
pub const GRAPH_AUDIO_BUS_FLOOR: usize = 32;
/// Control-rate counterpart of [`GRAPH_AUDIO_BUS_FLOOR`].
pub const GRAPH_CONTROL_BUS_FLOOR: usize = 128;

/// **How much of a bus space is private to GraphDef instances**, given how
/// many buses the server was configured with: half of it, never less than the
/// floor and never more than the space.
///
/// A share and not a number, because it is a *partition* of a configured
/// resource: a server booted with more buses has to hand the extra ones to
/// both sides, or raising the count would not buy a piece a single track. That
/// is exactly what a fixed 32 did -- a piece spends four private buses per
/// track and one or two per clip, so a multitrack ran out at seven tracks and
/// no configuration could change it.
///
/// Half, because the two sides are the two ways buses get used and neither is
/// the minor one: a graph's private wiring is the bulk of a mixer, and a
/// client's own buses are what everything outside one is patched with.
///
/// Lives here -- not in the server -- so a client subtracts the same
/// reservation the server it is talking to was built with, computed from the
/// count that server reports rather than agreed on by convention.
pub fn graph_audio_reserved(audio_buses: usize) -> usize {
    (audio_buses / 2)
        .max(GRAPH_AUDIO_BUS_FLOOR)
        .min(audio_buses)
}

/// Control-rate counterpart of [`graph_audio_reserved`].
pub fn graph_control_reserved(control_buses: usize) -> usize {
    (control_buses / 2)
        .max(GRAPH_CONTROL_BUS_FLOOR)
        .min(control_buses)
}

/// The boot-derived partition of the node-id space, every range scaled from
/// the engine's node-table capacity (`--max-nodes`) — the one resource that
/// actually bounds concurrent nodes. Id 0 is the root group; ids below
/// `client_base` stay reserved for well-known client use (scsynth
/// convention). The server reports the client range over `/server_query`, so
/// a client sizes its registry by query, not convention.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeIdPartition {
    /// First id a client's registry hands out.
    pub client_base: i64,
    /// Client id-space size: node-table capacity with in-flight margin — ids
    /// allocated whose `/synth_new` or `/node_end` is still travelling.
    pub client_capacity: usize,
    /// First id of the server's auto range (`/synth_new -1`, GraphDef members).
    pub auto_base: i64,
    pub auto_capacity: usize,
    /// First id of the server's MIDI-voice range.
    pub midi_base: i64,
    pub midi_capacity: usize,
}

impl NodeIdPartition {
    /// The partition for a node table of `max_nodes` slots.
    pub fn from_max_nodes(max_nodes: usize) -> Self {
        let max_nodes = max_nodes.max(1);
        let client_base = 1000;
        let client_capacity = 4 * max_nodes;
        let auto_base = client_base + client_capacity as i64;
        let auto_capacity = 2 * max_nodes;
        let midi_base = auto_base + auto_capacity as i64;
        Self {
            client_base,
            client_capacity,
            auto_base,
            auto_capacity,
            midi_base,
            midi_capacity: 2 * max_nodes,
        }
    }
}

/// Why a [`Registry::release`] was refused. The refused range is untouched —
/// a failed release never clears a partial run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseError {
    /// Some of the ids fall outside `[base, base + capacity)`.
    OutOfRange,
    /// Some of the ids are not currently allocated (a double release, or an
    /// id this registry never handed out).
    NotAllocated,
}

#[derive(Clone, Debug)]
enum Space {
    /// The real thing: a fixed occupancy map, preallocated once.
    Bounded {
        used: Vec<bool>,
        /// Next-fit scan start: rotates over the space so freshly released
        /// ids are not immediately re-handed (kinder to debugging and to
        /// in-flight traffic), yet the scan stays O(1) amortized.
        hint: usize,
        in_use: usize,
    },
    /// NRT/score mode: inexhaustible by design (see the module doc).
    Unbounded { next: i64, live: usize },
}

/// A registry of one finite id space `[base, base + capacity)`.
#[derive(Clone, Debug)]
pub struct Registry {
    base: i64,
    space: Space,
}

impl Registry {
    /// A bounded registry over `[base, base + capacity)`.
    pub fn new(base: i64, capacity: usize) -> Self {
        Self {
            base,
            space: Space::Bounded {
                used: vec![false; capacity],
                hint: 0,
                in_use: 0,
            },
        }
    }

    /// The NRT/score-mode registry: allocation never fails, ids ascend from
    /// `base`, `release` only maintains the live count.
    pub fn unbounded(base: i64) -> Self {
        Self {
            base,
            space: Space::Unbounded {
                next: base,
                live: 0,
            },
        }
    }

    /// The first id of the space.
    pub fn base(&self) -> i64 {
        self.base
    }

    /// The size of the id space; `None` when unbounded.
    pub fn capacity(&self) -> Option<usize> {
        match &self.space {
            Space::Bounded { used, .. } => Some(used.len()),
            Space::Unbounded { .. } => None,
        }
    }

    /// How many ids are currently allocated.
    pub fn in_use(&self) -> usize {
        match &self.space {
            Space::Bounded { in_use, .. } => *in_use,
            Space::Unbounded { live, .. } => *live,
        }
    }

    /// Whether `id` falls inside this registry's space (allocated or not).
    /// The filter for foreign `/node_end` ids: releases outside the space are
    /// another owner's business.
    pub fn contains(&self, id: i64) -> bool {
        match &self.space {
            Space::Bounded { used, .. } => id >= self.base && id < self.base + used.len() as i64,
            Space::Unbounded { .. } => id >= self.base,
        }
    }

    /// Whether `id` is currently allocated. Always `true` inside an unbounded
    /// space (it has no occupancy map to consult).
    pub fn is_allocated(&self, id: i64) -> bool {
        match &self.space {
            Space::Bounded { used, .. } => self.contains(id) && used[(id - self.base) as usize],
            Space::Unbounded { next, .. } => id >= self.base && id < *next,
        }
    }

    /// Allocates `width` contiguous ids and returns the first, or `None` when
    /// no such run exists — exhaustion is an explicit failure, never a wrap.
    /// `width` 0 counts as 1.
    pub fn alloc(&mut self, width: usize) -> Option<i64> {
        let w = width.max(1);
        match &mut self.space {
            Space::Bounded { used, hint, in_use } => {
                let start =
                    Self::find_run(used, *hint, w).or_else(|| Self::find_run(used, 0, w))?;
                used[start..start + w].iter_mut().for_each(|b| *b = true);
                *hint = start + w;
                *in_use += w;
                Some(self.base + start as i64)
            }
            Space::Unbounded { next, live } => {
                let first = *next;
                *next += w as i64;
                *live += w;
                Some(first)
            }
        }
    }

    /// Returns `width` ids starting at `first` to the pool. Refuses — leaving
    /// the map untouched — if any id is out of range or not allocated, so a
    /// double release (or a foreign id) is reported, never absorbed into a
    /// corrupt map. `width` 0 counts as 1.
    pub fn release(&mut self, first: i64, width: usize) -> Result<(), ReleaseError> {
        let w = width.max(1);
        match &mut self.space {
            Space::Bounded { used, in_use, .. } => {
                if first < self.base || first + w as i64 > self.base + used.len() as i64 {
                    return Err(ReleaseError::OutOfRange);
                }
                let start = (first - self.base) as usize;
                if used[start..start + w].iter().any(|&b| !b) {
                    return Err(ReleaseError::NotAllocated);
                }
                used[start..start + w].iter_mut().for_each(|b| *b = false);
                *in_use -= w;
                Ok(())
            }
            Space::Unbounded { next, live } => {
                if first < self.base || first + (w as i64) > *next {
                    return Err(ReleaseError::OutOfRange);
                }
                *live = live.saturating_sub(w);
                Ok(())
            }
        }
    }

    /// **Marks a specific run allocated**, answering whether it could: every id
    /// of `[first, first + width)` inside the space and free. Refused whole —
    /// nothing is marked — otherwise. `width` 0 counts as 1.
    ///
    /// What moving live allocations into a narrower space takes: the ids a
    /// client already holds are claimed in the new map at the numbers they
    /// already have, since the server already knows them by those numbers.
    pub fn claim(&mut self, first: i64, width: usize) -> bool {
        let w = width.max(1);
        match &mut self.space {
            Space::Bounded { used, in_use, .. } => {
                if first < self.base || first + w as i64 > self.base + used.len() as i64 {
                    return false;
                }
                let start = (first - self.base) as usize;
                if used[start..start + w].iter().any(|&b| b) {
                    return false;
                }
                used[start..start + w].iter_mut().for_each(|b| *b = true);
                *in_use += w;
                true
            }
            Space::Unbounded { next, live } => {
                if first < self.base {
                    return false;
                }
                *next = (*next).max(first + w as i64);
                *live += w;
                true
            }
        }
    }

    /// Releases everything (a client reset / `/group_freeAll`-scale event).
    pub fn clear(&mut self) {
        match &mut self.space {
            Space::Bounded { used, hint, in_use } => {
                used.iter_mut().for_each(|b| *b = false);
                *hint = 0;
                *in_use = 0;
            }
            Space::Unbounded { live, .. } => *live = 0,
        }
    }

    /// First free run of `w` at or after `from`; skips ahead past the last
    /// occupied slot found, so the scan is linear over the map, not quadratic.
    fn find_run(used: &[bool], from: usize, w: usize) -> Option<usize> {
        let n = used.len();
        let mut i = from;
        while i + w <= n {
            match (i..i + w).rev().find(|&j| used[j]) {
                None => return Some(i),
                Some(j) => i = j + 1,
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_ranges_are_disjoint_and_scale_from_max_nodes() {
        let p = NodeIdPartition::from_max_nodes(1024);
        assert_eq!(p.client_base, 1000);
        assert_eq!(p.client_capacity, 4096);
        assert_eq!(p.auto_base, 1000 + 4096);
        assert_eq!(p.midi_base, p.auto_base + p.auto_capacity as i64);
        // Ranges never overlap, whatever the table size.
        for n in [1, 7, 1024, 65536] {
            let p = NodeIdPartition::from_max_nodes(n);
            assert!(p.client_base + (p.client_capacity as i64) <= p.auto_base);
            assert!(p.auto_base + (p.auto_capacity as i64) <= p.midi_base);
            assert!(p.client_base > 0, "root and well-known ids stay below");
        }
    }

    #[test]
    fn allocates_from_base_and_tracks_use() {
        let mut r = Registry::new(1000, 4);
        assert_eq!(r.alloc(1), Some(1000));
        assert_eq!(r.alloc(1), Some(1001));
        assert_eq!(r.in_use(), 2);
        assert_eq!(r.capacity(), Some(4));
        assert!(r.is_allocated(1000));
        assert!(!r.is_allocated(1002));
    }

    #[test]
    fn exhaustion_is_none_never_a_wrap() {
        let mut r = Registry::new(0, 2);
        assert_eq!(r.alloc(1), Some(0));
        assert_eq!(r.alloc(1), Some(1));
        assert_eq!(r.alloc(1), None);
        // Nothing was lost: releasing makes the same space allocatable again.
        r.release(0, 1).unwrap();
        assert_eq!(r.alloc(1), Some(0));
    }

    #[test]
    fn every_release_is_reusable() {
        let mut r = Registry::new(0, 3);
        // Cycle far past capacity: with release-before-alloc the space never
        // exhausts — the no-monotonic-counter invariant in action.
        for _ in 0..100 {
            let id = r
                .alloc(1)
                .expect("space must never exhaust while recycling");
            r.release(id, 1).unwrap();
        }
        assert_eq!(r.in_use(), 0);
    }

    #[test]
    fn next_fit_rotates_instead_of_rehanding_the_freed_id() {
        let mut r = Registry::new(0, 4);
        let a = r.alloc(1).unwrap();
        r.release(a, 1).unwrap();
        // The freed id is reusable but not the immediate next choice.
        assert_ne!(r.alloc(1), Some(a));
    }

    #[test]
    fn contiguous_runs_and_coalescing() {
        let mut r = Registry::new(0, 8);
        let a = r.alloc(2).unwrap();
        let b = r.alloc(2).unwrap();
        let c = r.alloc(2).unwrap();
        assert_eq!((a, b, c), (0, 2, 4));
        // Free the two middle-adjacent runs: the map coalesces by nature and
        // a 4-wide run fits where two 2-wide ones lived.
        r.release(a, 2).unwrap();
        r.release(b, 2).unwrap();
        assert_eq!(r.alloc(4), Some(0));
    }

    #[test]
    fn double_release_and_foreign_ids_are_refused_atomically() {
        let mut r = Registry::new(10, 4);
        let a = r.alloc(2).unwrap();
        r.release(a, 2).unwrap();
        assert_eq!(r.release(a, 2), Err(ReleaseError::NotAllocated));
        assert_eq!(r.release(0, 1), Err(ReleaseError::OutOfRange));
        assert_eq!(r.release(13, 2), Err(ReleaseError::OutOfRange));
        // Partial overlap (one allocated, one not) must not clear the
        // allocated half.
        let b = r.alloc(1).unwrap();
        assert_eq!(r.release(b, 2), Err(ReleaseError::NotAllocated));
        assert!(r.is_allocated(b));
    }

    #[test]
    fn unbounded_never_exhausts_and_counts_live() {
        let mut r = Registry::unbounded(1000);
        let a = r.alloc(1).unwrap();
        let b = r.alloc(3).unwrap();
        assert_eq!((a, b), (1000, 1001));
        assert_eq!(r.capacity(), None);
        assert_eq!(r.in_use(), 4);
        r.release(a, 1).unwrap();
        assert_eq!(r.in_use(), 3);
        assert_eq!(r.release(5000, 1), Err(ReleaseError::OutOfRange));
    }

    #[test]
    fn contains_filters_foreign_id_spaces() {
        let r = Registry::new(1000, 4096);
        assert!(r.contains(1000));
        assert!(r.contains(5095));
        assert!(!r.contains(5096));
        assert!(!r.contains(0));
    }

    #[test]
    fn clear_resets_the_whole_space() {
        let mut r = Registry::new(0, 2);
        r.alloc(2).unwrap();
        assert_eq!(r.alloc(1), None);
        r.clear();
        assert_eq!(r.in_use(), 0);
        assert_eq!(r.alloc(2), Some(0));
    }
}
