//! Node tree: groups and synths with their execution order.
//!
//! Everything here runs on the audio thread, so the tree is fully
//! pre-allocated: a fixed slab of [`MAX_NODES`] slots plus pre-reserved child
//! lists (inserts are rejected when a list is at capacity, never grown).
//! Slot 0 is the root group (node ID 0), which always exists and cannot be
//! freed or moved. Freed nodes are handed to a caller-provided sink, which
//! must route them to the garbage FIFO — nothing heap-allocated is dropped
//! here.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicI32, AtomicU8, AtomicUsize, Ordering};

use crate::dsp::{BusUsage, DoneAction, ProcessCtx};
use crate::server::workers::WorkerPool;

/// What the tree processes. Implemented by `synthdef::instance::UGenSynth`
/// (M3) and, in the F fork, by `FaustSynth` — both built off the audio thread
/// and shipped in as `Box<dyn SynthNode>`.
pub trait SynthNode: Send {
    /// Processes one block. Output happens through `Out` UGens writing to the
    /// buses in `ctx`. Runs on the audio thread: must not allocate, lock or
    /// do I/O.
    fn process(&mut self, ctx: &mut ProcessCtx);
    /// Unknown indices must be ignored, like scsynth.
    fn set_control(&mut self, index: u32, value: f32);
    /// Maps control `index` to a bus read at the start of every block:
    /// `/n_map` (a control bus, `audio = false`) or `/n_mapa` (an audio bus
    /// sampled at control rate, `audio = true`). `bus < 0` removes the
    /// mapping. Unknown indices are ignored, like scsynth. A later
    /// [`set_control`](Self::set_control) on the same index clears the mapping.
    fn map_control(&mut self, index: u32, bus: i32, audio: bool);
    /// How many UGens this synth contributes to `/status.reply`.
    fn ugen_count(&self) -> usize;
    /// The maximum done action returned by any of this synth's UGens
    fn done_action(&self) -> DoneAction {
        DoneAction::None
    }
}

/// One control's bus mapping (`/n_map`/`/n_mapa`), stored per synth parallel
/// to its controls. `bus < 0` is unmapped; `audio` selects the audio-bus
/// space (sampled at control rate, one frame per block) over the control-bus
/// space. The synth applies live mappings at the start of every block.
#[derive(Clone, Copy)]
pub struct ControlMap {
    pub bus: i32,
    pub audio: bool,
}

impl ControlMap {
    pub const UNMAPPED: Self = Self {
        bus: -1,
        audio: false,
    };
}

/// Fixed capacity of the node slab (scsynth's `-n` option; configurable later).
pub const MAX_NODES: usize = 1024;
/// Pre-reserved child capacity of non-root groups.
pub const MAX_GROUP_CHILDREN: usize = 256;
/// Root node ID, like scsynth.
pub const ROOT_NODE_ID: i32 = 0;

const ROOT_SLOT: usize = 0;
const NO_PARENT: usize = usize::MAX;

/// Where to insert a new node, with the same numbering as scsynth's
/// `addAction` argument.
#[derive(Clone, Copy, Debug)]
pub enum AddAction {
    /// 0: first child of the target group.
    Head,
    /// 1: last child of the target group.
    Tail,
    /// 2: just before the target node, same parent.
    Before,
    /// 3: just after the target node, same parent.
    After,
    /// 4: take the target node's place; the target (and its subtree) is freed.
    Replace,
}

impl AddAction {
    pub fn from_i32(v: i32) -> Option<Self> {
        match v {
            0 => Some(Self::Head),
            1 => Some(Self::Tail),
            2 => Some(Self::Before),
            3 => Some(Self::After),
            4 => Some(Self::Replace),
            _ => None,
        }
    }
}

/// Where to move an existing node relative to a sibling (`/n_before`,
/// `/n_after`).
#[derive(Clone, Copy, Debug)]
pub enum Place {
    Before,
    After,
}

pub enum NodeKind {
    Synth {
        node: Box<dyn SynthNode>,
        /// Bus usage analyzed by the network thread at build time (M13):
        /// the parallel scheduler partitions stages from this.
        usage: BusUsage,
    },
    Group(Group),
}

pub struct Group {
    /// Slot indices of the children, in execution order. Pre-allocated; the
    /// tree rejects inserts that would grow it.
    pub children: Vec<usize>,
    /// `/g_parallel` (M13): children run in dependency stages on the worker
    /// pool instead of strictly in order.
    pub parallel: bool,
}

impl Group {
    /// For non-root groups, built on the network thread.
    pub fn new() -> Self {
        Self {
            children: Vec::with_capacity(MAX_GROUP_CHILDREN),
            parallel: false,
        }
    }
}

impl Default for Group {
    fn default() -> Self {
        Self::new()
    }
}

/// A node leaving the tree, reported through the free sink. Carries the
/// parent's node ID for `/n_end` notifications.
pub enum FreedNode {
    Synth {
        id: i32,
        parent_id: i32,
        synth: Box<dyn SynthNode>,
    },
    Group {
        id: i32,
        parent_id: i32,
        group: Group,
    },
}

struct NodeSlot {
    id: i32,
    parent: usize,
    kind: NodeKind,
    /// Paused by `DoneAction::PauseSelf`, a `FreeSelfPause{Prev,Next}`, or
    /// `/n_run 0`: the node stays in the tree and keeps its state but is skipped
    /// during processing (silent). A paused **group** skips its whole subtree.
    /// Cleared by `/n_run 1` (or `FreeSelfResumeNext` on the next sibling).
    paused: bool,
}

impl Default for NodeTree {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: the `UnsafeCell`s in `slots` are only accessed concurrently
// during `process`, where the M13 stage scheduler hands **disjoint
// subtrees** to the workers — each slot is reached by exactly one thread
// per slice. Every other method takes `&mut self`.
unsafe impl Sync for NodeTree {}

pub struct NodeTree {
    slots: Vec<UnsafeCell<Option<NodeSlot>>>,
    /// Pre-allocated stack for the depth-first processing traversal.
    dfs_stack: Vec<usize>,
    /// Pre-allocated stack for recursive frees: (slot, parent node ID at the
    /// time of unlinking — the parent slot may already be gone by then).
    free_stack: Vec<(usize, i32)>,
    synth_count: usize,
    group_count: usize,
    ugen_count: usize,
    /// Lock-free queue of nodes that finished this block with a freeing/relative
    /// done action (everything but `None`/`PauseSelf`); `done_actions` is the
    /// parallel action code per entry, applied by `apply_done_action` in the
    /// drain. `PauseSelf` is applied inline, not queued.
    done_nodes: Vec<AtomicI32>,
    done_actions: Vec<AtomicU8>,
    done_count: AtomicUsize,
}

impl NodeTree {
    pub fn new() -> Self {
        let mut slots: Vec<UnsafeCell<Option<NodeSlot>>> =
            (0..MAX_NODES).map(|_| UnsafeCell::new(None)).collect();
        *slots[ROOT_SLOT].get_mut() = Some(NodeSlot {
            id: ROOT_NODE_ID,
            parent: NO_PARENT,
            kind: NodeKind::Group(Group {
                children: Vec::with_capacity(MAX_NODES),
                parallel: false,
            }),
            paused: false,
        });
        Self {
            slots,
            dfs_stack: Vec::with_capacity(MAX_NODES),
            free_stack: Vec::with_capacity(MAX_NODES),
            synth_count: 0,
            group_count: 1,
            ugen_count: 0,
            done_nodes: (0..MAX_NODES).map(|_| AtomicI32::new(0)).collect(),
            done_actions: (0..MAX_NODES).map(|_| AtomicU8::new(0)).collect(),
            done_count: AtomicUsize::new(0),
        }
    }

    pub fn synth_count(&self) -> usize {
        self.synth_count
    }

    pub fn group_count(&self) -> usize {
        self.group_count
    }

    pub fn ugen_count(&self) -> usize {
        self.ugen_count
    }

    /// Resets the finished-node queue and returns how many IDs it holds; read
    /// each with [`Self::done_node`]. Two accessors rather than a draining
    /// closure so the caller can `free` each node between reads without a
    /// second borrow of the tree. The count may exceed [`MAX_NODES`] if a
    /// split block re-queued a node — the extra reads just miss (a `free` of an
    /// already-gone id is a no-op), so it is clamped here.
    ///
    /// Relaxed is enough: the queue is written under the parallel scheduler and
    /// drained only after the worker pool's join, whose happens-before edge
    /// publishes every store; the atomics just keep the concurrent
    /// index-reservation (`fetch_add`) race-free.
    pub fn take_done_count(&mut self) -> usize {
        self.done_count.swap(0, Ordering::Relaxed).min(MAX_NODES)
    }

    /// One finished-node ID from the queue captured by [`Self::take_done_count`].
    pub fn done_node(&self, i: usize) -> i32 {
        self.done_nodes[i].load(Ordering::Relaxed)
    }

    /// The freeing action for queue entry `i` (any freeing/relative action; not
    /// `None`/`PauseSelf`, which are applied inline).
    pub fn done_action_at(&self, i: usize) -> DoneAction {
        DoneAction::from_u8(self.done_actions[i].load(Ordering::Relaxed))
    }

    /// Records a node that finished with a freeing action, for the engine to
    /// drain after the walk. Takes `&self` (interior-mutable atomics) so the
    /// concurrent workers can push while holding their slot.
    #[inline]
    fn enqueue_done(&self, id: i32, action: DoneAction) {
        let c = self.done_count.fetch_add(1, Ordering::Relaxed);
        if c < MAX_NODES {
            self.done_nodes[c].store(id, Ordering::Relaxed);
            self.done_actions[c].store(action as u8, Ordering::Relaxed);
        }
    }

    /// Shared view of a slot. Sound outside `process` (no concurrency);
    /// during `process` only on slots the calling thread owns.
    #[inline]
    fn slot(&self, idx: usize) -> Option<&NodeSlot> {
        unsafe { (*self.slots[idx].get()).as_ref() }
    }

    #[inline]
    fn slot_mut(&mut self, idx: usize) -> Option<&mut NodeSlot> {
        self.slots[idx].get_mut().as_mut()
    }

    #[inline]
    fn take_slot(&mut self, idx: usize) -> Option<NodeSlot> {
        self.slots[idx].get_mut().take()
    }

    fn find(&self, id: i32) -> Option<usize> {
        (0..self.slots.len()).find(|&i| self.slot(i).is_some_and(|s| s.id == id))
    }

    fn id_of(&self, idx: usize) -> i32 {
        self.slot(idx).map_or(-1, |s| s.id)
    }

    fn group_of(&self, idx: usize) -> Option<&Group> {
        match &self.slot(idx)?.kind {
            NodeKind::Group(g) => Some(g),
            NodeKind::Synth { .. } => None,
        }
    }

    fn group_of_mut(&mut self, idx: usize) -> Option<&mut Group> {
        match &mut self.slot_mut(idx)?.kind {
            NodeKind::Group(g) => Some(g),
            NodeKind::Synth { .. } => None,
        }
    }

    /// Removes `idx` from its parent's child list.
    fn unlink(&mut self, idx: usize) {
        let Some(parent) = self.slot(idx).map(|s| s.parent) else {
            return;
        };
        if parent == NO_PARENT {
            return;
        }
        if let Some(g) = self.group_of_mut(parent)
            && let Some(pos) = g.children.iter().position(|&c| c == idx)
        {
            g.children.remove(pos);
        }
    }

    /// True if `a` is `b` or an ancestor of `b`.
    fn is_ancestor_or_self(&self, a: usize, mut b: usize) -> bool {
        loop {
            if a == b {
                return true;
            }
            match self.slot(b) {
                Some(s) if s.parent != NO_PARENT => b = s.parent,
                _ => return false,
            }
        }
    }

    /// Frees the subtree rooted at `idx` (already unlinked from its parent),
    /// reporting every node to `sink` parent-first — the order `/n_end`
    /// notifications go out in.
    fn free_subtree(&mut self, idx: usize, parent_id: i32, sink: &mut dyn FnMut(FreedNode)) {
        debug_assert!(self.free_stack.is_empty());
        self.free_stack.push((idx, parent_id));
        while let Some((idx, parent_id)) = self.free_stack.pop() {
            let Some(slot) = self.take_slot(idx) else {
                continue;
            };
            match slot.kind {
                NodeKind::Synth { node: synth, .. } => {
                    self.synth_count -= 1;
                    self.ugen_count -= synth.ugen_count();
                    sink(FreedNode::Synth {
                        id: slot.id,
                        parent_id,
                        synth,
                    });
                }
                NodeKind::Group(group) => {
                    self.group_count -= 1;
                    for &c in &group.children {
                        self.free_stack.push((c, slot.id));
                    }
                    sink(FreedNode::Group {
                        id: slot.id,
                        parent_id,
                        group,
                    });
                }
            }
        }
    }

    /// Inserts a node relative to `target` according to `action`. On success
    /// returns the parent group's node ID (for `/n_go`). On failure
    /// (duplicate ID, unknown target, full slab/group, type mismatch) returns
    /// the node back so the caller can dispose of it RT-safely. A `Replace`
    /// frees the target's subtree through `sink`.
    pub fn insert(
        &mut self,
        id: i32,
        kind: NodeKind,
        target: i32,
        action: AddAction,
        sink: &mut dyn FnMut(FreedNode),
    ) -> Result<i32, NodeKind> {
        if id == ROOT_NODE_ID || self.find(id).is_some() {
            return Err(kind);
        }
        let Some(tidx) = self.find(target) else {
            return Err(kind);
        };
        let Some(free) = (0..self.slots.len()).find(|&i| self.slot(i).is_none()) else {
            return Err(kind);
        };

        // Resolve parent and position, validating capacity before mutating.
        let (parent_idx, pos, replaces) = match action {
            AddAction::Head | AddAction::Tail => {
                let Some(g) = self.group_of(tidx) else {
                    return Err(kind); // target is not a group
                };
                if g.children.len() >= g.children.capacity() {
                    return Err(kind);
                }
                let pos = match action {
                    AddAction::Head => 0,
                    _ => g.children.len(),
                };
                (tidx, pos, None)
            }
            AddAction::Before | AddAction::After => {
                let parent = self.slot(tidx).map(|s| s.parent).unwrap();
                if parent == NO_PARENT {
                    return Err(kind); // target is the root group
                }
                let g = self.group_of(parent).unwrap();
                if g.children.len() >= g.children.capacity() {
                    return Err(kind);
                }
                let at = g.children.iter().position(|&c| c == tidx).unwrap();
                let pos = match action {
                    AddAction::After => at + 1,
                    _ => at,
                };
                (parent, pos, None)
            }
            AddAction::Replace => {
                let parent = self.slot(tidx).map(|s| s.parent).unwrap();
                if parent == NO_PARENT {
                    return Err(kind); // the root group cannot be replaced
                }
                let g = self.group_of(parent).unwrap();
                let pos = g.children.iter().position(|&c| c == tidx).unwrap();
                (parent, pos, Some(tidx))
            }
        };

        if let Some(tidx) = replaces {
            let parent_id = self.id_of(parent_idx);
            self.unlink(tidx);
            self.free_subtree(tidx, parent_id, sink);
        }

        match &kind {
            NodeKind::Synth { node, .. } => {
                self.synth_count += 1;
                self.ugen_count += node.ugen_count();
            }
            NodeKind::Group(_) => self.group_count += 1,
        }
        // After a Replace, `free` may have opened up earlier slots; the one
        // found above is still vacant either way.
        *self.slots[free].get_mut() = Some(NodeSlot {
            id,
            parent: parent_idx,
            kind,
            paused: false,
        });
        let g = self.group_of_mut(parent_idx).unwrap();
        let pos = pos.min(g.children.len());
        g.children.insert(pos, free);
        Ok(self.id_of(parent_idx))
    }

    /// Frees a node and (for groups) its whole subtree. Returns `false` if
    /// the ID does not exist or is the root group.
    pub fn free(&mut self, id: i32, sink: &mut dyn FnMut(FreedNode)) -> bool {
        if id == ROOT_NODE_ID {
            return false;
        }
        let Some(idx) = self.find(id) else {
            return false;
        };
        let parent = self.slot(idx).unwrap().parent;
        let parent_id = self.id_of(parent);
        self.unlink(idx);
        self.free_subtree(idx, parent_id, sink);
        true
    }

    /// `DoneAction::FreeGroup` (scsynth 14): frees the group that encloses node
    /// `id` (and its whole subtree, `id` included). If the enclosing group is
    /// the un-freeable root, frees just the node itself as a fallback. Returns
    /// `false` if the node does not exist.
    pub fn free_enclosing_group(&mut self, id: i32, sink: &mut dyn FnMut(FreedNode)) -> bool {
        let Some(idx) = self.find(id) else {
            return false;
        };
        let parent = self.slot(idx).unwrap().parent;
        let parent_id = self.id_of(parent);
        if parent == NO_PARENT || parent_id == ROOT_NODE_ID {
            return self.free(id, sink);
        }
        self.free(parent_id, sink)
    }

    /// `/g_freeAll`: frees every child of the group (recursively); the group
    /// itself stays.
    pub fn free_all(&mut self, group_id: i32, sink: &mut dyn FnMut(FreedNode)) -> bool {
        let Some(idx) = self.find(group_id) else {
            return false;
        };
        if self.group_of(idx).is_none() {
            return false;
        }
        loop {
            let Some(child) = self.group_of_mut(idx).unwrap().children.pop() else {
                return true;
            };
            self.free_subtree(child, group_id, sink);
        }
    }

    /// `/g_deepFree`: frees every synth in the group and its subgroups; the
    /// groups all stay.
    pub fn deep_free(&mut self, group_id: i32, sink: &mut dyn FnMut(FreedNode)) -> bool {
        let Some(idx) = self.find(group_id) else {
            return false;
        };
        if self.group_of(idx).is_none() {
            return false;
        }
        debug_assert!(self.dfs_stack.is_empty());
        self.dfs_stack.clear();
        self.dfs_stack.push(idx);
        while let Some(gidx) = self.dfs_stack.pop() {
            let gid = self.id_of(gidx);
            let mut i = 0;
            while let Some(&child) = self.group_of(gidx).unwrap().children.get(i) {
                match self.slot(child).map(|s| &s.kind) {
                    Some(NodeKind::Synth { .. }) => {
                        self.group_of_mut(gidx).unwrap().children.remove(i);
                        let slot = self.take_slot(child).unwrap();
                        if let NodeKind::Synth { node: synth, .. } = slot.kind {
                            self.synth_count -= 1;
                            self.ugen_count -= synth.ugen_count();
                            sink(FreedNode::Synth {
                                id: slot.id,
                                parent_id: gid,
                                synth,
                            });
                        }
                    }
                    Some(NodeKind::Group(_)) => {
                        self.dfs_stack.push(child);
                        i += 1;
                    }
                    None => i += 1,
                }
            }
        }
        true
    }

    /// The node ID of the sibling `delta` positions from node `id` within its
    /// parent group (`-1` = preceding, `+1` = following), or `None` at an edge
    /// or for the root. RT-safe: index arithmetic over the child list.
    fn sibling_id(&self, id: i32, delta: isize) -> Option<i32> {
        let idx = self.find(id)?;
        let parent = self.slot(idx)?.parent;
        if parent == NO_PARENT {
            return None;
        }
        let g = self.group_of(parent)?;
        let pos = g.children.iter().position(|&c| c == idx)?;
        let target = pos.checked_add_signed(delta)?;
        g.children.get(target).map(|&c| self.id_of(c))
    }

    fn is_group(&self, id: i32) -> bool {
        self.find(id)
            .is_some_and(|idx| self.group_of(idx).is_some())
    }

    /// Pauses (`paused = true`) or resumes (`false`) a node — a synth or a
    /// whole group. `/n_run` and the pause/resume done actions route here.
    /// Returns `false` if the ID is unknown. Never allocates.
    pub fn set_paused(&mut self, id: i32, paused: bool) -> bool {
        match self.find(id).and_then(|idx| self.slot_mut(idx)) {
            Some(slot) => {
                slot.paused = paused;
                true
            }
            None => false,
        }
    }

    /// Applies a queued freeing/relative [`DoneAction`] (everything except
    /// `None`/`PauseSelf`, which are handled inline during the walk): frees this
    /// node and, per the action, its previous/next sibling, the run of nodes to
    /// the group's head/tail, or the enclosing group — or pauses/resumes/
    /// deep-frees a neighbour. Runs on the audio thread during the done drain,
    /// so it only reuses the allocation-free `free`/`free_all`/`deep_free`
    /// machinery (the pre-allocated stacks). Neighbours are resolved *before*
    /// self is freed, since freeing shifts positions.
    pub fn apply_done_action(
        &mut self,
        id: i32,
        action: DoneAction,
        sink: &mut dyn FnMut(FreedNode),
    ) {
        use DoneAction::*;
        match action {
            None | PauseSelf => {} // applied inline, never queued
            FreeSelf => {
                self.free(id, sink);
            }
            FreeGroup => {
                self.free_enclosing_group(id, sink);
            }
            FreeAllInGroup => {
                // Free this node and every other node in its group.
                if let Some(idx) = self.find(id) {
                    let parent_id = self.id_of(self.slot(idx).unwrap().parent);
                    self.free_all(parent_id, sink);
                }
            }
            FreeSelfAndPrev => {
                let prev = self.sibling_id(id, -1);
                self.free(id, sink);
                if let Some(p) = prev {
                    self.free(p, sink);
                }
            }
            FreeSelfAndNext => {
                let next = self.sibling_id(id, 1);
                self.free(id, sink);
                if let Some(n) = next {
                    self.free(n, sink);
                }
            }
            FreeSelfToHead => {
                while let Some(prev) = self.sibling_id(id, -1) {
                    self.free(prev, sink);
                }
                self.free(id, sink);
            }
            FreeSelfToTail => {
                while let Some(next) = self.sibling_id(id, 1) {
                    self.free(next, sink);
                }
                self.free(id, sink);
            }
            FreeSelfPausePrev => {
                let prev = self.sibling_id(id, -1);
                self.free(id, sink);
                if let Some(p) = prev {
                    self.set_paused(p, true);
                }
            }
            FreeSelfPauseNext => {
                let next = self.sibling_id(id, 1);
                self.free(id, sink);
                if let Some(n) = next {
                    self.set_paused(n, true);
                }
            }
            FreeSelfResumeNext => {
                let next = self.sibling_id(id, 1);
                self.free(id, sink);
                if let Some(n) = next {
                    self.set_paused(n, false);
                }
            }
            FreeSelfAndFreeAllInPrev => {
                let prev = self.sibling_id(id, -1);
                self.free(id, sink);
                if let Some(p) = prev {
                    self.free_or_free_all(p, sink);
                }
            }
            FreeSelfAndFreeAllInNext => {
                let next = self.sibling_id(id, 1);
                self.free(id, sink);
                if let Some(n) = next {
                    self.free_or_free_all(n, sink);
                }
            }
            FreeSelfAndDeepFreePrev => {
                let prev = self.sibling_id(id, -1);
                self.free(id, sink);
                if let Some(p) = prev {
                    self.free_or_deep_free(p, sink);
                }
            }
            FreeSelfAndDeepFreeNext => {
                let next = self.sibling_id(id, 1);
                self.free(id, sink);
                if let Some(n) = next {
                    self.free_or_deep_free(n, sink);
                }
            }
        }
    }

    /// A neighbour node: if it is a group, free its children (`free_all`); else
    /// free the node itself.
    fn free_or_free_all(&mut self, id: i32, sink: &mut dyn FnMut(FreedNode)) {
        if self.is_group(id) {
            self.free_all(id, sink);
        } else {
            self.free(id, sink);
        }
    }

    /// A neighbour node: if it is a group, deep-free it; else free the node.
    fn free_or_deep_free(&mut self, id: i32, sink: &mut dyn FnMut(FreedNode)) {
        if self.is_group(id) {
            self.deep_free(id, sink);
        } else {
            self.free(id, sink);
        }
    }

    /// `/n_before` / `/n_after`: moves a node next to a sibling, possibly
    /// under a different parent. Rejects moves of/into the node's own
    /// subtree, of the root, and into a full group.
    pub fn move_node(&mut self, id: i32, target: i32, place: Place) -> bool {
        if id == ROOT_NODE_ID || id == target {
            return false;
        }
        let (Some(idx), Some(tidx)) = (self.find(id), self.find(target)) else {
            return false;
        };
        let new_parent = self.slot(tidx).unwrap().parent;
        if new_parent == NO_PARENT {
            return false; // cannot be a sibling of the root group
        }
        if self.is_ancestor_or_self(idx, new_parent) {
            return false; // would create a cycle
        }
        let old_parent = self.slot(idx).unwrap().parent;
        if new_parent != old_parent {
            let g = self.group_of(new_parent).unwrap();
            if g.children.len() >= g.children.capacity() {
                return false;
            }
        }
        self.unlink(idx);
        let g = self.group_of_mut(new_parent).unwrap();
        let Some(at) = g.children.iter().position(|&c| c == tidx) else {
            return false; // unreachable: target verified above
        };
        let pos = match place {
            Place::Before => at,
            Place::After => at + 1,
        };
        g.children.insert(pos, idx);
        self.slot_mut(idx).unwrap().parent = new_parent;
        true
    }

    pub fn synth_mut(&mut self, id: i32) -> Option<&mut dyn SynthNode> {
        let idx = self.find(id)?;
        match &mut self.slot_mut(idx)?.kind {
            NodeKind::Synth { node, .. } => Some(node.as_mut()),
            NodeKind::Group(_) => None,
        }
    }

    /// Updates a synth's bus-usage masks (`Cmd::SetUsage`, after an `/n_set`
    /// on a control used as a bus index).
    pub fn set_usage(&mut self, id: i32, usage: BusUsage) {
        if let Some(idx) = self.find(id)
            && let Some(slot) = self.slot_mut(idx)
            && let NodeKind::Synth { usage: u, .. } = &mut slot.kind
        {
            *u = usage;
        }
    }

    /// Flags a group as parallel (`/g_parallel`). Returns `false` when the
    /// ID is missing or not a group.
    pub fn set_parallel(&mut self, id: i32, parallel: bool) -> bool {
        let Some(idx) = self.find(id) else {
            return false;
        };
        match self.slot_mut(idx).map(|s| &mut s.kind) {
            Some(NodeKind::Group(g)) => {
                g.parallel = parallel;
                true
            }
            _ => false,
        }
    }

    /// Depth-first traversal in execution order; synths write to the buses in
    /// `ctx` through their I/O UGens. Runs on the audio thread: no
    /// allocation. Children of groups flagged parallel (`/g_parallel`) run
    /// in dependency **stages** on the worker pool (M13).
    pub fn process(&mut self, ctx: &ProcessCtx, pool: &WorkerPool) {
        // SAFETY: entry point — this thread owns the whole tree; the pool
        // only ever receives disjoint subtrees.
        unsafe { self.process_index(ROOT_SLOT, ctx, pool) }
    }

    /// Processes the subtree rooted at slot `idx`, dispatching parallel
    /// stages to the pool.
    ///
    /// # Safety
    /// During the current slice, `idx`'s subtree must be visited by exactly
    /// one thread (the stage scheduler guarantees this for workers).
    pub(crate) unsafe fn process_index(&self, idx: usize, ctx: &ProcessCtx, pool: &WorkerPool) {
        // SAFETY: per the contract, no other thread touches this slot now.
        let Some(slot) = (unsafe { &mut *self.slots[idx].get() }).as_mut() else {
            return;
        };
        // A paused node is skipped whole — a synth stays silent, a group skips
        // its entire subtree.
        if slot.paused {
            return;
        }
        match &mut slot.kind {
            NodeKind::Synth { node, .. } => {
                let mut ctx = *ctx;
                node.process(&mut ctx);
                match node.done_action() {
                    DoneAction::None => {}
                    DoneAction::PauseSelf => slot.paused = true,
                    action => self.enqueue_done(slot.id, action),
                }
            }
            NodeKind::Group(group) if group.parallel => unsafe {
                self.process_parallel(&group.children, ctx, pool);
            },
            NodeKind::Group(group) => {
                for &child in &group.children {
                    unsafe { self.process_index(child, ctx, pool) };
                }
            }
        }
    }

    /// Sequential variant for worker threads: identical traversal, but
    /// nested parallel groups run inline (no nested fork-join on one pool).
    ///
    /// # Safety
    /// Same single-visitor contract as [`Self::process_index`].
    pub(crate) unsafe fn process_index_seq(&self, idx: usize, ctx: &ProcessCtx) {
        // SAFETY: per the contract, no other thread touches this slot now.
        let Some(slot) = (unsafe { &mut *self.slots[idx].get() }).as_mut() else {
            return;
        };
        if slot.paused {
            return;
        }
        match &mut slot.kind {
            NodeKind::Synth { node, .. } => {
                let mut ctx = *ctx;
                node.process(&mut ctx);
                match node.done_action() {
                    DoneAction::None => {}
                    DoneAction::PauseSelf => slot.paused = true,
                    action => self.enqueue_done(slot.id, action),
                }
            }
            NodeKind::Group(group) => {
                for &child in &group.children {
                    unsafe { self.process_index_seq(child, ctx) };
                }
            }
        }
    }

    /// Greedy stage partition of a parallel group's children, in order:
    /// a child joins the current stage while it writes nothing the stage
    /// reads or writes and reads nothing the stage writes; conflicts close
    /// the stage (those children run after — sequential semantics
    /// preserved); a `dynamic` child (signal-driven bus index) always runs
    /// alone. Since stage members touch pairwise disjoint buses and stages
    /// run in child order, the output is **bit-identical** to sequential
    /// execution regardless of worker interleaving.
    ///
    /// # Safety
    /// Single-visitor contract on the subtree (see [`Self::process_index`]).
    unsafe fn process_parallel(&self, children: &[usize], ctx: &ProcessCtx, pool: &WorkerPool) {
        let mut i = 0;
        while i < children.len() {
            let mut reads = 0u128;
            let mut writes = 0u128;
            let mut j = i;
            while j < children.len() {
                let usage = self.subtree_usage(children[j]);
                if usage.dynamic {
                    if j == i {
                        j += 1; // the dynamic child is its own stage
                    }
                    break;
                }
                if j > i && ((usage.writes & (reads | writes)) != 0 || (usage.reads & writes) != 0)
                {
                    break;
                }
                reads |= usage.reads;
                writes |= usage.writes;
                j += 1;
            }
            if j - i >= 2 {
                pool.run_stage(self, &children[i..j], ctx);
            } else {
                // SAFETY: contract propagates to the single child.
                unsafe { self.process_index(children[i], ctx, pool) };
            }
            i = j;
        }
    }

    /// A node's bus usage; for groups, the union over the subtree. Pure
    /// bitops over engine-owned masks — RT-safe.
    fn subtree_usage(&self, idx: usize) -> BusUsage {
        match self.slot(idx).map(|s| &s.kind) {
            Some(NodeKind::Synth { usage, .. }) => *usage,
            Some(NodeKind::Group(g)) => g.children.iter().fold(BusUsage::default(), |acc, &c| {
                acc.union(self.subtree_usage(c))
            }),
            None => BusUsage::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A structural-only synth: it never produces sound, it just carries a done
    /// action so the tree tests can drive `apply_done_action`.
    struct MockSynth {
        done: DoneAction,
    }

    impl SynthNode for MockSynth {
        fn process(&mut self, _ctx: &mut ProcessCtx) {}
        fn set_control(&mut self, _index: u32, _value: f32) {}
        fn map_control(&mut self, _index: u32, _bus: i32, _audio: bool) {}
        fn ugen_count(&self) -> usize {
            1
        }
        fn done_action(&self) -> DoneAction {
            self.done
        }
    }

    fn add_synth(tree: &mut NodeTree, id: i32, parent: i32) {
        let kind = NodeKind::Synth {
            node: Box::new(MockSynth {
                done: DoneAction::None,
            }),
            usage: BusUsage::default(),
        };
        assert!(
            tree.insert(id, kind, parent, AddAction::Tail, &mut |_| {})
                .is_ok(),
            "insert synth {id}"
        );
    }

    fn add_group(tree: &mut NodeTree, id: i32, parent: i32) {
        let kind = NodeKind::Group(Group::new());
        assert!(
            tree.insert(id, kind, parent, AddAction::Tail, &mut |_| {})
                .is_ok(),
            "insert group {id}"
        );
    }

    fn alive(tree: &NodeTree, id: i32) -> bool {
        tree.find(id).is_some()
    }

    fn is_paused(tree: &NodeTree, id: i32) -> bool {
        tree.find(id)
            .and_then(|idx| tree.slot(idx))
            .is_some_and(|s| s.paused)
    }

    /// Fires a queued done action from `id` and returns the freed node IDs.
    fn fire(tree: &mut NodeTree, id: i32, action: DoneAction) -> Vec<i32> {
        let mut freed = Vec::new();
        tree.apply_done_action(id, action, &mut |f| {
            freed.push(match f {
                FreedNode::Synth { id, .. } => id,
                FreedNode::Group { id, .. } => id,
            });
        });
        freed
    }

    /// Root with three sibling synths 1, 2, 3 (2 is the usual actor).
    fn tree_1_2_3() -> NodeTree {
        let mut t = NodeTree::new();
        add_synth(&mut t, 1, ROOT_NODE_ID);
        add_synth(&mut t, 2, ROOT_NODE_ID);
        add_synth(&mut t, 3, ROOT_NODE_ID);
        t
    }

    #[test]
    fn free_self_leaves_siblings() {
        let mut t = tree_1_2_3();
        let freed = fire(&mut t, 2, DoneAction::FreeSelf);
        assert_eq!(freed, vec![2]);
        assert!(alive(&t, 1) && alive(&t, 3) && !alive(&t, 2));
    }

    #[test]
    fn free_self_and_prev_and_next() {
        let mut t = tree_1_2_3();
        fire(&mut t, 2, DoneAction::FreeSelfAndPrev);
        assert!(alive(&t, 3) && !alive(&t, 1) && !alive(&t, 2));

        let mut t = tree_1_2_3();
        fire(&mut t, 2, DoneAction::FreeSelfAndNext);
        assert!(alive(&t, 1) && !alive(&t, 2) && !alive(&t, 3));
    }

    #[test]
    fn free_self_to_head_and_to_tail() {
        // [1,2,3,4]; from 3 to head frees 3,2,1 -> only 4 remains.
        let mut t = tree_1_2_3();
        add_synth(&mut t, 4, ROOT_NODE_ID);
        fire(&mut t, 3, DoneAction::FreeSelfToHead);
        assert!(alive(&t, 4) && !alive(&t, 1) && !alive(&t, 2) && !alive(&t, 3));

        // [1,2,3,4]; from 2 to tail frees 2,3,4 -> only 1 remains.
        let mut t = tree_1_2_3();
        add_synth(&mut t, 4, ROOT_NODE_ID);
        fire(&mut t, 2, DoneAction::FreeSelfToTail);
        assert!(alive(&t, 1) && !alive(&t, 2) && !alive(&t, 3) && !alive(&t, 4));
    }

    #[test]
    fn free_all_in_group_clears_the_group() {
        let mut t = tree_1_2_3();
        fire(&mut t, 2, DoneAction::FreeAllInGroup);
        assert!(!alive(&t, 1) && !alive(&t, 2) && !alive(&t, 3));
        assert_eq!(t.synth_count(), 0);
    }

    #[test]
    fn free_group_frees_the_enclosing_group() {
        // root -> group 10 -> [1,2,3]; FreeGroup from 2 frees 10 and its subtree.
        let mut t = NodeTree::new();
        add_group(&mut t, 10, ROOT_NODE_ID);
        add_synth(&mut t, 1, 10);
        add_synth(&mut t, 2, 10);
        add_synth(&mut t, 3, 10);
        fire(&mut t, 2, DoneAction::FreeGroup);
        assert!(!alive(&t, 10) && !alive(&t, 1) && !alive(&t, 2) && !alive(&t, 3));
    }

    #[test]
    fn free_self_pause_next_then_resume() {
        let mut t = tree_1_2_3();
        fire(&mut t, 2, DoneAction::FreeSelfPauseNext);
        assert!(!alive(&t, 2) && alive(&t, 1) && alive(&t, 3));
        assert!(is_paused(&t, 3) && !is_paused(&t, 1));
        // A later FreeSelfResumeNext from 1 unpauses its next sibling (3).
        fire(&mut t, 1, DoneAction::FreeSelfResumeNext);
        assert!(!alive(&t, 1) && alive(&t, 3) && !is_paused(&t, 3));
    }

    #[test]
    fn free_all_in_next_group_when_neighbour_is_a_group() {
        // root -> [synth 1, group 10 -> {11, 12}]; from 1, free self and freeAll
        // the next group: 11, 12 gone, group 10 stays (empty).
        let mut t = NodeTree::new();
        add_synth(&mut t, 1, ROOT_NODE_ID);
        add_group(&mut t, 10, ROOT_NODE_ID);
        add_synth(&mut t, 11, 10);
        add_synth(&mut t, 12, 10);
        fire(&mut t, 1, DoneAction::FreeSelfAndFreeAllInNext);
        assert!(!alive(&t, 1) && alive(&t, 10) && !alive(&t, 11) && !alive(&t, 12));
    }

    #[test]
    fn deep_free_next_group_keeps_subgroups() {
        // root -> [synth 1, group 10 -> subgroup 11 -> synth 12]; from 1,
        // deep-free the next group: synth 12 gone, groups 10 and 11 stay.
        let mut t = NodeTree::new();
        add_synth(&mut t, 1, ROOT_NODE_ID);
        add_group(&mut t, 10, ROOT_NODE_ID);
        add_group(&mut t, 11, 10);
        add_synth(&mut t, 12, 11);
        fire(&mut t, 1, DoneAction::FreeSelfAndDeepFreeNext);
        assert!(!alive(&t, 1) && alive(&t, 10) && alive(&t, 11) && !alive(&t, 12));
    }

    #[test]
    fn set_paused_round_trips_and_rejects_unknown() {
        let mut t = tree_1_2_3();
        assert!(t.set_paused(2, true) && is_paused(&t, 2));
        assert!(t.set_paused(2, false) && !is_paused(&t, 2));
        assert!(!t.set_paused(999, true)); // unknown id
    }

    #[test]
    fn sibling_resolution_at_the_edges() {
        let t = tree_1_2_3();
        assert_eq!(t.sibling_id(2, -1), Some(1));
        assert_eq!(t.sibling_id(2, 1), Some(3));
        assert_eq!(t.sibling_id(1, -1), None); // first child: no prev
        assert_eq!(t.sibling_id(3, 1), None); // last child: no next
        assert_eq!(t.sibling_id(ROOT_NODE_ID, -1), None); // root has no siblings
    }
}
