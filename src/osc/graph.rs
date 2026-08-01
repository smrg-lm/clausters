//! Bus-connection analysis and auto-sorted groups (M12).
//!
//! The execution order problem: a node reading an audio bus must run *after*
//! the nodes writing it, and scsynth makes the client manage that order by
//! hand (`addAction`, `/node_before`). This module infers the dependency DAG
//! from the defs themselves — which audio buses each node reads (`In`,
//! Faust `in`) and writes (`Out`/`ReplaceOut`, Faust `out`) — and keeps
//! **opt-in auto-sorted groups** (`/group_sortMode`) in topological order, so
//! groups behave like the channels of a multitrack editor.
//!
//! Everything runs on the network thread against [`TreeMirror`], a mirror of
//! the node tree maintained from the same commands the engine receives; the
//! audio thread is untouched (re-sorts are ordinary `Cmd::MoveNode`s). The
//! mirror reflects commands **as sent**: commands inside a not-yet-fired
//! timed bundle are mirrored immediately, so the mirror can run briefly
//! ahead of the engine — re-sorts converge once the bundle fires.
//!
//! Analysis rules:
//! - A bus index that is a **constant or a control** is static and
//!   contributes edges (writer before reader). A `/node_set` on a control used
//!   as a bus index re-analyzes and re-sorts.
//! - A bus index computed by a **signal** makes the node *dynamic*: a
//!   conservative barrier that keeps its position, with nothing sorted
//!   across it.
//! - `ReplaceOut` counts as read+write (it consumes what is on the bus), so
//!   it orders after the summing writers it replaces and before the readers.
//! - **Cycles** (legitimate read-before-write feedback) are not "solved":
//!   the nodes involved keep their current relative order — one block of
//!   delay, exactly like a return send in a multitrack editor.

use std::collections::HashMap;

use crate::dsp::NUM_AUDIO_BUSES;
use crate::node::{AddAction, Place, ROOT_NODE_ID};
#[cfg(feature = "synth")]
use crate::synthdef::{InputRef, SynthDef};

#[cfg(feature = "synth")]
use crate::dsp::registry::BusRole;
#[cfg(feature = "faust")]
use crate::faust::synth::FaustDef;

/// Bus-usage masks now live in [`crate::dsp`] (the engine's parallel
/// scheduler uses them too, M13); re-exported here for the analysis API.
pub use crate::dsp::BusUsage;

/// Analyzes a UGen def against a node's current control values. Returns the
/// usage plus the control indices that act as bus indexes (a `/node_set` on one
/// of those must re-run the analysis).
#[cfg(feature = "synth")]
pub fn ugen_usage(def: &SynthDef, controls: &[f32]) -> (BusUsage, Vec<u32>) {
    let mut usage = BusUsage::default();
    let mut bus_controls = Vec::new();
    for ugen in &def.ugens {
        let (read, write) = match ugen.desc.bus {
            BusRole::Read => (true, false),
            BusRole::Write => (false, true),
            BusRole::ReadWrite => (true, true),
            BusRole::None => continue,
        };
        match ugen.inputs[0] {
            InputRef::Const(c) => usage.mark(def.constants[c], read, write),
            InputRef::Control(c) => {
                bus_controls.push(c as u32);
                usage.mark(controls.get(c).copied().unwrap_or(0.0), read, write);
            }
            InputRef::Wire(_) => usage.dynamic = true,
        }
    }
    (usage, bus_controls)
}

/// Faust synths read `in..in+inputs` and sum into `out..out+outputs`; the
/// two reserved controls sit after the UI params (see `faust::synth`), so
/// they are always static bus indexes.
#[cfg(feature = "faust")]
pub fn faust_usage(def: &FaustDef, controls: &[f32]) -> (BusUsage, Vec<u32>) {
    let np = def.params.len();
    let mut usage = BusUsage::default();
    let first = |value: f32, width: usize| {
        // Same clamp as `FaustSynth`'s `clamp_first_bus` + per-channel `min`.
        let max_first = NUM_AUDIO_BUSES - width.max(1);
        (value.max(0.0) as usize).min(max_first)
    };
    let out = first(controls.get(np).copied().unwrap_or(0.0), def.num_outputs);
    for i in 0..def.num_outputs {
        usage.writes |= 1 << (out + i).min(NUM_AUDIO_BUSES - 1);
    }
    let inb = first(controls.get(np + 1).copied().unwrap_or(0.0), def.num_inputs);
    for i in 0..def.num_inputs {
        usage.reads |= 1 << (inb + i).min(NUM_AUDIO_BUSES - 1);
    }
    (usage, vec![np as u32, np as u32 + 1])
}

/// Stable topological sort of a group's children by bus dependencies.
///
/// `units` is (node ID, usage) in the **current** order. Edge: `a` before
/// `b` when `a` writes a bus `b` reads. Dynamic units are barriers (ordered
/// against everything by current position). Kahn's algorithm picking the
/// earliest ready unit keeps the sort stable; on a cycle, the earliest
/// remaining unit is released — cycle members keep their current relative
/// order.
pub fn stable_topo_sort(units: &[(i32, BusUsage)]) -> Vec<i32> {
    let n = units.len();
    let mut before = vec![false; n * n];
    let edge = |i: usize, j: usize| i * n + j;
    for i in 0..n {
        for j in 0..n {
            if i != j && units[i].1.writes & units[j].1.reads != 0 {
                before[edge(i, j)] = true;
            }
        }
    }
    for d in 0..n {
        if units[d].1.dynamic {
            for i in 0..d {
                before[edge(i, d)] = true;
            }
            for j in d + 1..n {
                before[edge(d, j)] = true;
            }
        }
    }
    let mut indegree = vec![0usize; n];
    for i in 0..n {
        for j in 0..n {
            if before[i * n + j] {
                indegree[j] += 1;
            }
        }
    }
    let mut placed = vec![false; n];
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let pick = (0..n)
            .find(|&i| !placed[i] && indegree[i] == 0)
            // Deadlock = cycle: release the earliest remaining unit.
            .or_else(|| (0..n).find(|&i| !placed[i]))
            .expect("n unplaced units remain");
        placed[pick] = true;
        out.push(units[pick].0);
        for j in 0..n {
            if before[pick * n + j] && !placed[j] && indegree[j] > 0 {
                indegree[j] -= 1;
            }
        }
    }
    out
}

// ---- the network-side tree mirror ----

pub enum MirrorBody {
    Group {
        children: Vec<i32>,
        /// `/group_sortMode`: re-sort on every topology or bus-usage change.
        auto: bool,
        /// `/group_parallel` (M13): mirrored for `/group_dumpGraph` introspection.
        parallel: bool,
        /// `/group_name`: an optional label on top of the node ID, unique among
        /// the group's siblings. It never replaces the ID — every command still
        /// addresses the group by ID — but it names one segment of the group's
        /// path, which is what `/group_query` resolves. Lives here, on the
        /// network thread, and nowhere else: the engine's `NodeTree` has no
        /// notion of a name, so naming costs the audio thread nothing.
        name: Option<Box<str>>,
    },
    Synth {
        def_name: String,
        /// Current control values (defaults, then `/synth_new` and `/node_set`).
        controls: Vec<f32>,
        usage: BusUsage,
        /// Control indices used as bus indexes; `/node_set` on these re-sorts.
        bus_controls: Vec<u32>,
        /// Active `/node_map`/`/node_mapAudio` bindings as `(control, bus, audio)`.
        /// An audio map adds the bus to the node's reads; mapping a
        /// `bus_controls` index makes the node a dynamic barrier (see
        /// [`TreeMirror::fold_maps_into_usage`]).
        maps: Vec<(u32, i32, bool)>,
    },
}

impl MirrorBody {
    /// An empty, unnamed group body; `auto` marks the ones the translator
    /// builds for a graph instance, which sort themselves.
    pub fn group(auto: bool) -> Self {
        Self::Group {
            children: Vec::new(),
            auto,
            parallel: false,
            name: None,
        }
    }
}

pub struct MirrorNode {
    pub parent: i32,
    pub body: MirrorBody,
}

/// Network-side mirror of the engine's node tree, fed by the same `Cmd`
/// stream. Best-effort by design: a command the engine later rejects is
/// rolled back when the rejection comes home through the garbage FIFO
/// ([`remove`](TreeMirror::remove) is idempotent for that reason).
pub struct TreeMirror {
    nodes: HashMap<i32, MirrorNode>,
    /// The names of groups that have left the tree but whose `/node_end` has
    /// not gone out yet. The mirror drops a node when its command is
    /// *translated*, while the notification only leaves once the engine
    /// confirms the death, so a label has to outlive its entry by exactly that
    /// gap for the death notice to be able to name what died. Bounded: an
    /// epitaph nothing ever claims (the node was already gone engine-side, so
    /// no event follows) is pushed out by the ones after it.
    epitaphs: std::collections::VecDeque<(i32, Box<str>)>,
}

/// How many unclaimed epitaphs the mirror keeps. Comfortably more than the
/// deaths that can be in flight between the network thread and one engine
/// block, and small enough that the linear scan claiming one is free.
const MAX_EPITAPHS: usize = 256;

impl Default for TreeMirror {
    fn default() -> Self {
        Self::new()
    }
}

impl TreeMirror {
    pub fn new() -> Self {
        let mut nodes = HashMap::new();
        nodes.insert(
            ROOT_NODE_ID,
            MirrorNode {
                parent: ROOT_NODE_ID,
                body: MirrorBody::group(false),
            },
        );
        Self {
            nodes,
            epitaphs: std::collections::VecDeque::new(),
        }
    }

    /// Remembers a departing group's name for its `/node_end`.
    fn bury(&mut self, id: i32, name: Box<str>) {
        if self.epitaphs.len() >= MAX_EPITAPHS {
            self.epitaphs.pop_front();
        }
        self.epitaphs.push_back((id, name));
    }

    /// Claims the name of a group that has left the tree, for the death
    /// notification. `None` for a synth, an unnamed group, or a death whose
    /// epitaph was already claimed or pushed out.
    pub fn take_epitaph(&mut self, id: i32) -> Option<Box<str>> {
        let at = self.epitaphs.iter().rposition(|(other, _)| *other == id)?;
        self.epitaphs.remove(at).map(|(_, name)| name)
    }

    pub fn get(&self, id: i32) -> Option<&MirrorNode> {
        self.nodes.get(&id)
    }

    pub fn children(&self, group: i32) -> Option<&[i32]> {
        match &self.nodes.get(&group)?.body {
            MirrorBody::Group { children, .. } => Some(children),
            MirrorBody::Synth { .. } => None,
        }
    }

    pub fn is_auto_group(&self, id: i32) -> bool {
        matches!(
            self.nodes.get(&id).map(|n| &n.body),
            Some(MirrorBody::Group { auto: true, .. })
        )
    }

    /// The parent group, or `None` for the root (and for unknown nodes).
    pub fn parent(&self, id: i32) -> Option<i32> {
        if id == ROOT_NODE_ID {
            return None;
        }
        self.nodes.get(&id).map(|n| n.parent)
    }

    pub fn set_auto(&mut self, group: i32, auto: bool) -> Result<(), String> {
        match self.nodes.get_mut(&group).map(|n| &mut n.body) {
            Some(MirrorBody::Group { auto: flag, .. }) => {
                *flag = auto;
                Ok(())
            }
            Some(MirrorBody::Synth { .. }) => Err(format!("node {group} is not a group")),
            None => Err(format!("group {group} not found")),
        }
    }

    pub fn set_parallel(&mut self, group: i32, parallel: bool) -> Result<(), String> {
        match self.nodes.get_mut(&group).map(|n| &mut n.body) {
            Some(MirrorBody::Group { parallel: flag, .. }) => {
                *flag = parallel;
                Ok(())
            }
            Some(MirrorBody::Synth { .. }) => Err(format!("node {group} is not a group")),
            None => Err(format!("group {group} not found")),
        }
    }

    /// A group's name, or `""` when it has none (and for a synth or an unknown
    /// node). The empty string is how every reply says "unnamed": a group with
    /// no name reports no name, never its ID — the ID stands in for the name
    /// only when composing a path, so that no group falls out of addressing.
    pub fn name_of(&self, id: i32) -> &str {
        match self.nodes.get(&id).map(|n| &n.body) {
            Some(MirrorBody::Group { name, .. }) => name.as_deref().unwrap_or(""),
            _ => "",
        }
    }

    /// The rules a label obeys, in one place so `/group_name` and a
    /// `/group_new` that carries a name enforce exactly the same ones: unique
    /// among the segments of the group's siblings under `parent`, never all
    /// digits (that would collide with the ID segment of another group) and
    /// never carrying a `/` (the server composes paths, the client does not).
    /// `group` is the node being named, skipped in the sibling scan; `None`
    /// for a group that does not exist yet.
    fn check_name(&self, parent: i32, group: Option<i32>, name: &str) -> Result<(), String> {
        if name.contains('/') {
            return Err("a group name cannot contain '/'".into());
        }
        if name.bytes().all(|b| b.is_ascii_digit()) {
            return Err("a group name cannot be all digits".into());
        }
        let taken = self
            .children(parent)
            .unwrap_or(&[])
            .iter()
            .any(|&sib| Some(sib) != group && self.segment_matches(sib, name));
        if taken {
            return Err(format!("name '{name}' is already taken in that group"));
        }
        Ok(())
    }

    /// Validates the label a `/group_new` carries **before** the group is
    /// created, against the group it would land in. A name the server refuses
    /// refuses the whole creation: a client that asked for a named group would
    /// otherwise be left with an anonymous one it never asked for, and would
    /// have to query the tree to find out.
    pub fn check_new_name(&self, target: i32, action: AddAction, name: &str) -> Result<(), String> {
        // Where the group would land, by the same rule `insert` applies. An
        // unresolvable target is left to `insert` and to the engine, which
        // reject it on their own terms; only the name is judged here.
        let parent = match action {
            AddAction::Head | AddAction::Tail => target,
            _ if target == ROOT_NODE_ID => return self.check_name(ROOT_NODE_ID, None, name),
            _ => match self.nodes.get(&target) {
                Some(node) => node.parent,
                None => return self.check_name(ROOT_NODE_ID, None, name),
            },
        };
        self.check_name(parent, None, name)
    }

    /// `/group_name`: labels a group, or clears the label when `name` is empty.
    /// Validated here, on the network thread, before anything is queued.
    pub fn set_name(&mut self, group: i32, name: &str) -> Result<(), String> {
        match self.nodes.get(&group).map(|n| &n.body) {
            Some(MirrorBody::Group { .. }) => {}
            Some(MirrorBody::Synth { .. }) => return Err(format!("node {group} is not a group")),
            None => return Err(format!("group {group} not found")),
        }
        if !name.is_empty() {
            let parent = self.nodes[&group].parent;
            self.check_name(parent, Some(group), name)?;
        }
        if let Some(MirrorBody::Group { name: slot, .. }) =
            self.nodes.get_mut(&group).map(|n| &mut n.body)
        {
            *slot = (!name.is_empty()).then(|| name.into());
        }
        Ok(())
    }

    /// Whether `node` answers to the path segment `seg`. Every node answers to
    /// its decimal ID — the ID is the identity and a name never takes its
    /// place — and a named group answers to its name as well. Which is why a
    /// name may not be all digits: it would speak for another node's ID.
    fn segment_matches(&self, node: i32, seg: &str) -> bool {
        if seg.parse::<i32>() == Ok(node) {
            return true;
        }
        matches!(
            self.nodes.get(&node).map(|n| &n.body),
            Some(MirrorBody::Group { name: Some(name), .. }) if &**name == seg
        )
    }

    /// The path of a node from the root: `/mixer/reverb`, with an unnamed
    /// group contributing its ID (`/1000/reverb`). The root itself is `/`.
    /// Composed on the walk, never stored, so renaming a group rewrites the
    /// path of its whole subtree at once.
    pub fn path_of(&self, id: i32) -> Option<String> {
        if !self.nodes.contains_key(&id) {
            return None;
        }
        let mut segments = Vec::new();
        let mut node = id;
        while node != ROOT_NODE_ID {
            match self.nodes.get(&node).map(|n| &n.body) {
                Some(MirrorBody::Group {
                    name: Some(name), ..
                }) => segments.push(name.to_string()),
                _ => segments.push(node.to_string()),
            }
            node = self.nodes[&node].parent;
        }
        segments.reverse();
        Some(format!("/{}", segments.join("/")))
    }

    /// `/group_query`: the node a path names, or `None` when no node answers to
    /// it. `/` is the root group; a leading slash is optional.
    pub fn resolve_path(&self, path: &str) -> Option<i32> {
        let mut node = ROOT_NODE_ID;
        for seg in path.split('/').filter(|s| !s.is_empty()) {
            node = *self
                .children(node)?
                .iter()
                .find(|&&child| self.segment_matches(child, seg))?;
        }
        Some(node)
    }

    pub fn is_parallel_group(&self, id: i32) -> bool {
        matches!(
            self.nodes.get(&id).map(|n| &n.body),
            Some(MirrorBody::Group { parallel: true, .. })
        )
    }

    /// Mirrors `NodeTree::insert` (same placement rules; capacity limits are
    /// left to the engine — a rejection rolls the mirror back later).
    pub fn insert(
        &mut self,
        id: i32,
        body: MirrorBody,
        target: i32,
        action: AddAction,
    ) -> Result<i32, String> {
        if id == ROOT_NODE_ID || self.nodes.contains_key(&id) {
            return Err(format!("node {id} already exists"));
        }
        if !self.nodes.contains_key(&target) {
            return Err(format!("target {target} not found"));
        }
        let (parent, pos) = match action {
            AddAction::Head | AddAction::Tail => {
                let Some(children) = self.children(target) else {
                    return Err(format!("target {target} is not a group"));
                };
                let pos = match action {
                    AddAction::Head => 0,
                    _ => children.len(),
                };
                (target, pos)
            }
            AddAction::Before | AddAction::After => {
                if target == ROOT_NODE_ID {
                    return Err("target is the root group".into());
                }
                let parent = self.nodes[&target].parent;
                let at = self.position(parent, target);
                (
                    parent,
                    if matches!(action, AddAction::After) {
                        at + 1
                    } else {
                        at
                    },
                )
            }
            AddAction::Replace => {
                if target == ROOT_NODE_ID {
                    return Err("the root group cannot be replaced".into());
                }
                let parent = self.nodes[&target].parent;
                let pos = self.position(parent, target);
                self.remove(target);
                (parent, pos)
            }
        };
        self.nodes.insert(id, MirrorNode { parent, body });
        if let Some(MirrorBody::Group { children, .. }) =
            self.nodes.get_mut(&parent).map(|n| &mut n.body)
        {
            children.insert(pos.min(children.len()), id);
        }
        Ok(parent)
    }

    fn position(&self, parent: i32, child: i32) -> usize {
        self.children(parent)
            .and_then(|c| c.iter().position(|&i| i == child))
            .unwrap_or(0)
    }

    /// Removes a node and its subtree. Idempotent: also called when engine
    /// garbage confirms (or rejects) something the mirror already dropped.
    pub fn remove(&mut self, id: i32) {
        if id == ROOT_NODE_ID {
            return;
        }
        let Some(node) = self.nodes.remove(&id) else {
            return;
        };
        if let MirrorBody::Group { children, name, .. } = node.body {
            if let Some(name) = name {
                self.bury(id, name);
            }
            for child in children {
                self.remove_subtree(child);
            }
        }
        if let Some(MirrorBody::Group { children, .. }) =
            self.nodes.get_mut(&node.parent).map(|n| &mut n.body)
        {
            children.retain(|&c| c != id);
        }
    }

    fn remove_subtree(&mut self, id: i32) {
        if let Some(node) = self.nodes.remove(&id)
            && let MirrorBody::Group { children, name, .. } = node.body
        {
            if let Some(name) = name {
                self.bury(id, name);
            }
            for child in children {
                self.remove_subtree(child);
            }
        }
    }

    /// `/group_freeAll`: drop all children, keep the group.
    pub fn free_all(&mut self, group: i32) {
        let Some(children) = self.children(group).map(<[i32]>::to_vec) else {
            return;
        };
        for child in children {
            self.remove(child);
        }
    }

    /// `/group_deepFree`: drop the synths of the group and its subgroups; the
    /// groups stay.
    pub fn deep_free(&mut self, group: i32) {
        let Some(children) = self.children(group).map(<[i32]>::to_vec) else {
            return;
        };
        for child in children {
            match self.nodes.get(&child).map(|n| &n.body) {
                Some(MirrorBody::Synth { .. }) => self.remove(child),
                Some(MirrorBody::Group { .. }) => self.deep_free(child),
                None => {}
            }
        }
    }

    /// Mirrors `NodeTree::move_node`. Returns the (old, new) parent groups.
    pub fn move_node(&mut self, id: i32, target: i32, place: Place) -> Option<(i32, i32)> {
        if id == ROOT_NODE_ID || id == target {
            return None;
        }
        if !self.nodes.contains_key(&id) || !self.nodes.contains_key(&target) {
            return None;
        }
        // For sibling moves the destination is the target's parent; for
        // head/tail the target is the destination group itself (and must be one).
        let new_parent = match place {
            Place::Before | Place::After => {
                if target == ROOT_NODE_ID {
                    return None;
                }
                self.nodes[&target].parent
            }
            Place::Head | Place::Tail => {
                if !matches!(self.nodes[&target].body, MirrorBody::Group { .. }) {
                    return None;
                }
                target
            }
        };
        if self.is_ancestor_or_self(id, new_parent) {
            return None;
        }
        let old_parent = self.nodes[&id].parent;
        if let Some(MirrorBody::Group { children, .. }) =
            self.nodes.get_mut(&old_parent).map(|n| &mut n.body)
        {
            children.retain(|&c| c != id);
        }
        let child_count = self
            .children(new_parent)
            .map(<[i32]>::len)
            .unwrap_or_default();
        let pos = match place {
            Place::Head => 0,
            Place::Tail => child_count,
            Place::Before => self.position(new_parent, target),
            Place::After => self.position(new_parent, target) + 1,
        };
        if let Some(MirrorBody::Group { children, .. }) =
            self.nodes.get_mut(&new_parent).map(|n| &mut n.body)
        {
            children.insert(pos.min(children.len()), id);
        }
        self.nodes.get_mut(&id).unwrap().parent = new_parent;
        Some((old_parent, new_parent))
    }

    fn is_ancestor_or_self(&self, node: i32, mut candidate: i32) -> bool {
        loop {
            if candidate == node {
                return true;
            }
            if candidate == ROOT_NODE_ID {
                return false;
            }
            candidate = self.nodes[&candidate].parent;
        }
    }

    /// A node's bus usage; for groups, the union over the whole subtree.
    pub fn usage_of(&self, id: i32) -> BusUsage {
        match self.nodes.get(&id).map(|n| &n.body) {
            Some(MirrorBody::Synth { usage, .. }) => *usage,
            Some(MirrorBody::Group { children, .. }) => children
                .clone()
                .iter()
                .fold(BusUsage::default(), |acc, &c| acc.union(self.usage_of(c))),
            None => BusUsage::default(),
        }
    }

    /// New child order for an auto group, or `None` when already sorted.
    pub fn sorted_children(&self, group: i32) -> Option<Vec<i32>> {
        let children = self.children(group)?;
        if children.len() < 2 {
            return None;
        }
        let units: Vec<(i32, BusUsage)> = children.iter().map(|&c| (c, self.usage_of(c))).collect();
        let desired = stable_topo_sort(&units);
        (desired != children).then_some(desired)
    }

    /// Overwrites a group's child order (the mirror side of applying the
    /// `Cmd::MoveNode` chain the caller emits to the engine).
    pub fn set_children_order(&mut self, group: i32, order: Vec<i32>) {
        if let Some(MirrorBody::Group { children, .. }) =
            self.nodes.get_mut(&group).map(|n| &mut n.body)
        {
            debug_assert_eq!(children.len(), order.len());
            *children = order;
        }
    }

    /// A synth's def name and current control values.
    pub fn synth_info(&self, id: i32) -> Option<(&str, &[f32])> {
        match self.nodes.get(&id).map(|n| &n.body) {
            Some(MirrorBody::Synth {
                def_name, controls, ..
            }) => Some((def_name, controls)),
            _ => None,
        }
    }

    /// Updates one mirrored control value. Returns `true` when the control
    /// is used as a bus index (the caller must re-analyze and re-sort).
    pub fn set_control(&mut self, id: i32, index: u32, value: f32) -> bool {
        if let Some(MirrorBody::Synth {
            controls,
            bus_controls,
            ..
        }) = self.nodes.get_mut(&id).map(|n| &mut n.body)
        {
            if let Some(slot) = controls.get_mut(index as usize) {
                *slot = value;
            }
            bus_controls.contains(&index)
        } else {
            false
        }
    }

    pub fn set_usage(&mut self, id: i32, new: BusUsage) {
        if let Some(MirrorBody::Synth { usage, .. }) = self.nodes.get_mut(&id).map(|n| &mut n.body)
        {
            *usage = new;
        }
    }

    /// Records (`bus >= 0`) or clears (`bus < 0`) a control→bus mapping.
    /// Returns whether the change can affect the node's bus usage — i.e. it
    /// touches an audio map (new or just-cleared) or a control used as a bus
    /// index — so the caller knows to re-analyze and re-sort.
    pub fn set_map(&mut self, id: i32, ctl: u32, bus: i32, audio: bool) -> bool {
        if let Some(MirrorBody::Synth {
            maps, bus_controls, ..
        }) = self.nodes.get_mut(&id).map(|n| &mut n.body)
        {
            let had_audio = maps.iter().any(|&(c, _, a)| c == ctl && a);
            maps.retain(|&(c, _, _)| c != ctl);
            if bus >= 0 {
                maps.push((ctl, bus, audio));
            }
            had_audio || (audio && bus >= 0) || bus_controls.contains(&ctl)
        } else {
            false
        }
    }

    /// Folds a synth's live mappings into its statically computed usage: each
    /// audio map adds the bus to `reads`, and any mapped bus-index control
    /// forces `dynamic` (the index is no longer statically known).
    pub fn fold_maps_into_usage(&self, id: i32, mut usage: BusUsage) -> BusUsage {
        if let Some(MirrorBody::Synth {
            maps, bus_controls, ..
        }) = self.nodes.get(&id).map(|n| &n.body)
        {
            for &(ctl, bus, audio) in maps {
                if audio && bus >= 0 {
                    usage.reads |= 1 << (bus as usize).min(NUM_AUDIO_BUSES - 1);
                }
                if bus_controls.contains(&ctl) {
                    usage.dynamic = true;
                }
            }
        }
        usage
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `/group_new id action target` on the mirror, unnamed.
    fn group(mirror: &mut TreeMirror, id: i32, target: i32) {
        mirror
            .insert(id, MirrorBody::group(false), target, AddAction::Tail)
            .unwrap();
    }

    #[test]
    fn an_unnamed_group_answers_to_its_id_in_a_path() {
        let mut m = TreeMirror::new();
        group(&mut m, 1000, ROOT_NODE_ID);
        group(&mut m, 1001, 1000);
        m.set_name(1001, "reverb").unwrap();
        assert_eq!(m.path_of(ROOT_NODE_ID).unwrap(), "/");
        assert_eq!(m.path_of(1000).unwrap(), "/1000");
        assert_eq!(m.path_of(1001).unwrap(), "/1000/reverb");
        assert_eq!(m.resolve_path("/1000/reverb"), Some(1001));
        // The id keeps working as a segment even for a named group: it is the
        // identity, and the name is the label on top of it.
        assert_eq!(m.resolve_path("/1000/1001"), Some(1001));
        assert_eq!(m.resolve_path("/1000/chorus"), None);
    }

    #[test]
    fn renaming_a_group_rewrites_the_paths_below_it() {
        let mut m = TreeMirror::new();
        group(&mut m, 1000, ROOT_NODE_ID);
        group(&mut m, 1001, 1000);
        m.set_name(1000, "mixer").unwrap();
        m.set_name(1001, "drums").unwrap();
        assert_eq!(m.path_of(1001).unwrap(), "/mixer/drums");
        m.set_name(1000, "board").unwrap();
        assert_eq!(m.path_of(1001).unwrap(), "/board/drums");
        assert_eq!(m.resolve_path("/board/drums"), Some(1001));
        assert_eq!(m.resolve_path("/mixer/drums"), None);
    }

    #[test]
    fn a_name_is_unique_among_siblings_only() {
        let mut m = TreeMirror::new();
        group(&mut m, 1000, ROOT_NODE_ID);
        group(&mut m, 1001, ROOT_NODE_ID);
        group(&mut m, 1002, 1000);
        group(&mut m, 1003, 1001);
        m.set_name(1000, "g1").unwrap();
        m.set_name(1001, "g2").unwrap();
        // The same name under two different parents: g1/mixer and g2/mixer.
        m.set_name(1002, "mixer").unwrap();
        m.set_name(1003, "mixer").unwrap();
        assert_eq!(m.path_of(1002).unwrap(), "/g1/mixer");
        assert_eq!(m.path_of(1003).unwrap(), "/g2/mixer");
        // But twice under the same one is refused.
        assert!(m.set_name(1001, "g1").is_err());
        assert_eq!(m.name_of(1001), "g2", "a refused name changes nothing");
    }

    #[test]
    fn a_new_group_s_name_is_judged_before_it_exists() {
        let mut m = TreeMirror::new();
        group(&mut m, 1000, ROOT_NODE_ID);
        m.set_name(1000, "mixer").unwrap();
        // Judged against the group it would land in: as a child of the root
        // the name is taken, as a child of 1000 it is free.
        assert!(
            m.check_new_name(ROOT_NODE_ID, AddAction::Tail, "mixer")
                .is_err()
        );
        assert!(m.check_new_name(1000, AddAction::Tail, "mixer").is_ok());
        // A sibling placement is judged against the target's parent.
        assert!(m.check_new_name(1000, AddAction::After, "mixer").is_err());
        assert!(
            m.check_new_name(ROOT_NODE_ID, AddAction::Tail, "1000")
                .is_err()
        );
    }

    #[test]
    fn a_name_is_neither_a_number_nor_a_path() {
        let mut m = TreeMirror::new();
        group(&mut m, 1000, ROOT_NODE_ID);
        // All digits would be ambiguous with another group's id segment.
        assert!(m.set_name(1000, "1001").is_err());
        assert!(m.set_name(1000, "a/b").is_err());
        assert!(m.set_name(1000, "8bit").is_ok(), "digits inside are fine");
    }

    #[test]
    fn clearing_a_name_frees_it_for_another_group() {
        let mut m = TreeMirror::new();
        group(&mut m, 1000, ROOT_NODE_ID);
        group(&mut m, 1001, ROOT_NODE_ID);
        m.set_name(1000, "mixer").unwrap();
        assert!(m.set_name(1001, "mixer").is_err());
        m.set_name(1000, "").unwrap();
        assert_eq!(m.name_of(1000), "");
        assert_eq!(m.path_of(1000).unwrap(), "/1000");
        m.set_name(1001, "mixer").unwrap();
        assert_eq!(m.resolve_path("/mixer"), Some(1001));
    }

    #[test]
    fn a_freed_group_takes_its_name_with_it() {
        let mut m = TreeMirror::new();
        group(&mut m, 1000, ROOT_NODE_ID);
        m.set_name(1000, "mixer").unwrap();
        m.remove(1000);
        assert_eq!(m.resolve_path("/mixer"), None);
        group(&mut m, 1001, ROOT_NODE_ID);
        m.set_name(1001, "mixer").unwrap();
        assert_eq!(m.resolve_path("/mixer"), Some(1001));
    }

    #[test]
    fn only_a_group_takes_a_name() {
        let mut m = TreeMirror::new();
        m.insert(
            1000,
            MirrorBody::Synth {
                def_name: "default".into(),
                controls: Vec::new(),
                usage: BusUsage::default(),
                bus_controls: Vec::new(),
                maps: Vec::new(),
            },
            ROOT_NODE_ID,
            AddAction::Tail,
        )
        .unwrap();
        assert!(m.set_name(1000, "voice").is_err());
        assert!(m.set_name(4242, "nowhere").is_err());
        assert_eq!(m.name_of(1000), "");
    }
}
