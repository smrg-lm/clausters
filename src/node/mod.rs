//! Node tree: groups and synths with their execution order.
//!
//! Everything here runs on the audio thread, so the tree is fully
//! pre-allocated: a fixed slab of [`MAX_NODES`] slots plus pre-reserved child
//! lists (inserts are rejected when a list is at capacity, never grown).
//! Slot 0 is the root group (node ID 0), which always exists and cannot be
//! freed or moved. Freed nodes are handed to a caller-provided sink, which
//! must route them to the garbage FIFO — nothing heap-allocated is dropped
//! here.

use crate::dsp::ProcessCtx;

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
    /// How many UGens this synth contributes to `/status.reply`.
    fn ugen_count(&self) -> usize;
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
    Synth(Box<dyn SynthNode>),
    Group(Group),
}

pub struct Group {
    /// Slot indices of the children, in execution order. Pre-allocated; the
    /// tree rejects inserts that would grow it.
    pub children: Vec<usize>,
}

impl Group {
    /// For non-root groups, built on the network thread.
    pub fn new() -> Self {
        Self {
            children: Vec::with_capacity(MAX_GROUP_CHILDREN),
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
}

impl Default for NodeTree {
    fn default() -> Self {
        Self::new()
    }
}

pub struct NodeTree {
    slots: Vec<Option<NodeSlot>>,
    /// Pre-allocated stack for the depth-first processing traversal.
    dfs_stack: Vec<usize>,
    /// Pre-allocated stack for recursive frees: (slot, parent node ID at the
    /// time of unlinking — the parent slot may already be gone by then).
    free_stack: Vec<(usize, i32)>,
    synth_count: usize,
    group_count: usize,
    ugen_count: usize,
}

impl NodeTree {
    pub fn new() -> Self {
        let mut slots: Vec<Option<NodeSlot>> = (0..MAX_NODES).map(|_| None).collect();
        slots[ROOT_SLOT] = Some(NodeSlot {
            id: ROOT_NODE_ID,
            parent: NO_PARENT,
            kind: NodeKind::Group(Group {
                children: Vec::with_capacity(MAX_NODES),
            }),
        });
        Self {
            slots,
            dfs_stack: Vec::with_capacity(MAX_NODES),
            free_stack: Vec::with_capacity(MAX_NODES),
            synth_count: 0,
            group_count: 1,
            ugen_count: 0,
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

    fn find(&self, id: i32) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| s.as_ref().is_some_and(|s| s.id == id))
    }

    fn id_of(&self, idx: usize) -> i32 {
        self.slots[idx].as_ref().map_or(-1, |s| s.id)
    }

    fn group_of(&self, idx: usize) -> Option<&Group> {
        match &self.slots[idx].as_ref()?.kind {
            NodeKind::Group(g) => Some(g),
            NodeKind::Synth(_) => None,
        }
    }

    fn group_of_mut(&mut self, idx: usize) -> Option<&mut Group> {
        match &mut self.slots[idx].as_mut()?.kind {
            NodeKind::Group(g) => Some(g),
            NodeKind::Synth(_) => None,
        }
    }

    /// Removes `idx` from its parent's child list.
    fn unlink(&mut self, idx: usize) {
        let Some(parent) = self.slots[idx].as_ref().map(|s| s.parent) else {
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
            match self.slots[b].as_ref() {
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
            let Some(slot) = self.slots[idx].take() else {
                continue;
            };
            match slot.kind {
                NodeKind::Synth(synth) => {
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
        let Some(free) = self.slots.iter().position(|s| s.is_none()) else {
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
                let parent = self.slots[tidx].as_ref().map(|s| s.parent).unwrap();
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
                let parent = self.slots[tidx].as_ref().map(|s| s.parent).unwrap();
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
            NodeKind::Synth(s) => {
                self.synth_count += 1;
                self.ugen_count += s.ugen_count();
            }
            NodeKind::Group(_) => self.group_count += 1,
        }
        // After a Replace, `free` may have opened up earlier slots; the one
        // found above is still vacant either way.
        self.slots[free] = Some(NodeSlot {
            id,
            parent: parent_idx,
            kind,
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
        let parent = self.slots[idx].as_ref().unwrap().parent;
        let parent_id = self.id_of(parent);
        self.unlink(idx);
        self.free_subtree(idx, parent_id, sink);
        true
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
                match self.slots[child].as_ref().map(|s| &s.kind) {
                    Some(NodeKind::Synth(_)) => {
                        self.group_of_mut(gidx).unwrap().children.remove(i);
                        let slot = self.slots[child].take().unwrap();
                        if let NodeKind::Synth(synth) = slot.kind {
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
        let new_parent = self.slots[tidx].as_ref().unwrap().parent;
        if new_parent == NO_PARENT {
            return false; // cannot be a sibling of the root group
        }
        if self.is_ancestor_or_self(idx, new_parent) {
            return false; // would create a cycle
        }
        let old_parent = self.slots[idx].as_ref().unwrap().parent;
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
        self.slots[idx].as_mut().unwrap().parent = new_parent;
        true
    }

    pub fn synth_mut(&mut self, id: i32) -> Option<&mut dyn SynthNode> {
        let idx = self.find(id)?;
        match &mut self.slots[idx].as_mut()?.kind {
            NodeKind::Synth(synth) => Some(synth.as_mut()),
            NodeKind::Group(_) => None,
        }
    }

    /// Depth-first traversal in execution order; synths write to the buses in
    /// `ctx` through their I/O UGens. Runs on the audio thread: no allocation.
    pub fn process(&mut self, ctx: &mut ProcessCtx) {
        self.dfs_stack.clear();
        if let Some(slot) = self.slots[ROOT_SLOT].as_ref()
            && let NodeKind::Group(g) = &slot.kind
        {
            for &c in g.children.iter().rev() {
                self.dfs_stack.push(c);
            }
        }
        while let Some(idx) = self.dfs_stack.pop() {
            let Some(slot) = self.slots[idx].as_mut() else {
                continue;
            };
            match &mut slot.kind {
                NodeKind::Synth(synth) => synth.process(ctx),
                NodeKind::Group(group) => {
                    for &c in group.children.iter().rev() {
                        self.dfs_stack.push(c);
                    }
                }
            }
        }
    }
}
