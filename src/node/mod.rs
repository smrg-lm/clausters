//! Node tree: groups and synths with their execution order.
//!
//! Everything here runs on the audio thread, so the tree is fully
//! pre-allocated: a fixed slab of [`MAX_NODES`] slots plus pre-reserved child
//! lists. Inserting moves an already-boxed synth into a free slot; freeing
//! hands the box back to the caller (which must route it to the garbage FIFO,
//! never drop it here).

pub mod default_synth;

use default_synth::DefaultSynth;

/// Fixed capacity of the node slab (scsynth's `-n` option; configurable later).
pub const MAX_NODES: usize = 1024;

/// Where to insert a new node. Before/after/replace arrive in M4.
#[derive(Clone, Copy, Debug)]
pub enum AddAction {
    Head,
    Tail,
}

pub enum NodeKind {
    Synth(Box<DefaultSynth>),
    /// Structurally supported; `/g_new` creates these starting in M4.
    Group(Group),
}

pub struct Group {
    /// Slot indices of the children, in execution order.
    pub children: Vec<usize>,
}

struct NodeSlot {
    id: i32,
    kind: NodeKind,
}

pub struct NodeTree {
    slots: Vec<Option<NodeSlot>>,
    /// Children of the root group (node ID 0), in execution order.
    root_children: Vec<usize>,
    /// Pre-allocated stack for the depth-first traversal.
    dfs_stack: Vec<usize>,
    synth_count: usize,
}

impl NodeTree {
    pub fn new() -> Self {
        Self {
            slots: (0..MAX_NODES).map(|_| None).collect(),
            root_children: Vec::with_capacity(MAX_NODES),
            dfs_stack: Vec::with_capacity(MAX_NODES),
            synth_count: 0,
        }
    }

    pub fn synth_count(&self) -> usize {
        self.synth_count
    }

    fn find(&self, id: i32) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| s.as_ref().is_some_and(|s| s.id == id))
    }

    /// Inserts into the root group. Returns the synth back on duplicate ID or
    /// full table so the caller can dispose of it RT-safely.
    pub fn insert_synth(
        &mut self,
        id: i32,
        synth: Box<DefaultSynth>,
        action: AddAction,
    ) -> Result<(), Box<DefaultSynth>> {
        if self.find(id).is_some() {
            return Err(synth);
        }
        let Some(free) = self.slots.iter().position(|s| s.is_none()) else {
            return Err(synth);
        };
        self.slots[free] = Some(NodeSlot {
            id,
            kind: NodeKind::Synth(synth),
        });
        match action {
            AddAction::Head => self.root_children.insert(0, free),
            AddAction::Tail => self.root_children.push(free),
        }
        self.synth_count += 1;
        Ok(())
    }

    /// Unlinks a synth and returns it for disposal. `None` if the ID does not
    /// exist or is a group.
    pub fn free_synth(&mut self, id: i32) -> Option<Box<DefaultSynth>> {
        let idx = self.find(id)?;
        if !matches!(self.slots[idx].as_ref()?.kind, NodeKind::Synth(_)) {
            return None;
        }
        if let Some(pos) = self.root_children.iter().position(|&c| c == idx) {
            self.root_children.remove(pos);
        }
        match self.slots[idx].take()?.kind {
            NodeKind::Synth(synth) => {
                self.synth_count -= 1;
                Some(synth)
            }
            NodeKind::Group(_) => unreachable!("checked above"),
        }
    }

    pub fn synth_mut(&mut self, id: i32) -> Option<&mut DefaultSynth> {
        let idx = self.find(id)?;
        match &mut self.slots[idx].as_mut()?.kind {
            NodeKind::Synth(synth) => Some(synth),
            NodeKind::Group(_) => None,
        }
    }

    /// Depth-first traversal in execution order; each synth's output is summed
    /// into `mix`. Runs on the audio thread: no allocation.
    pub fn process(&mut self, sample_rate: f32, mix: &mut [f32], scratch: &mut [f32]) {
        self.dfs_stack.clear();
        for &c in self.root_children.iter().rev() {
            self.dfs_stack.push(c);
        }
        while let Some(idx) = self.dfs_stack.pop() {
            let Some(slot) = self.slots[idx].as_mut() else {
                continue;
            };
            match &mut slot.kind {
                NodeKind::Synth(synth) => {
                    synth.process(sample_rate, scratch);
                    for (m, s) in mix.iter_mut().zip(scratch.iter()) {
                        *m += *s;
                    }
                }
                NodeKind::Group(group) => {
                    for &c in group.children.iter().rev() {
                        self.dfs_stack.push(c);
                    }
                }
            }
        }
    }
}
