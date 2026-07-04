//! OSC message → engine command translation, shared between the real-time
//! server ([`crate::osc::server`]) and the NRT renderer
//! ([`crate::server::render`]).
//!
//! [`CmdTranslator`] owns everything that turning a message into fully built
//! [`Cmd`]s requires: the def tables, the node→def mirror that resolves
//! `/n_set` control names, and the auto node-ID counter. It covers the
//! schedulable subset of the protocol (`/s_new`, node/group commands,
//! `/c_set`) plus the synchronous def-table commands (`/d_recv`, `/d_free`).
//! Buffer commands parse into NRT jobs with [`parse_buffer_msg`].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rosc::OscType;

use crate::dsp::buffer::{Buffer, BufferPool, empty_pool_with};
use crate::dsp::{
    Limits, MAX_UGEN_CMD_ARGS, NUM_AUDIO_BUSES, NUM_CONTROL_BUSES, UGenCmd, ugen_cmd_selector,
};
#[cfg(feature = "faust")]
use crate::faust::synth::{FaustDef, FaustSynth};
use crate::midi::{ChannelVoiceMessage, MidiBinding, MidiBindings, convert};
use crate::node::{AddAction, Group, Place, SynthNode};
#[cfg(feature = "synth")]
use crate::osc::graph::ugen_usage;
use crate::osc::graph::{BusUsage, MirrorBody, TreeMirror};
use crate::osc::graphdef::{
    BusRate, ControlValue, GRAPH_AUDIO_BUS_RESERVED, GRAPH_CONTROL_BUS_RESERVED, GraphDefSpec,
    GraphInstance, GraphVoice, RangeAllocator, ResolvedSurface,
};
use crate::server::engine::Cmd;
use crate::server::nrt::NrtJob;
#[cfg(feature = "synth")]
use crate::synthdef::instance::UGenSynth;
#[cfg(feature = "synth")]
use crate::synthdef::{SynthDef, SynthDefSpec, compile, default_spec};

#[cfg(feature = "faust")]
use crate::osc::graph::faust_usage;

/// Auto-assigned node IDs (`/s_new` with ID -1) start above this.
const AUTO_NODE_ID_BASE: i32 = 2_000_000;

/// What a live node was built from, mirrored per node ID so `/n_set` can
/// resolve control names off the audio thread.
#[derive(Clone)]
pub enum NodeDef {
    #[cfg(feature = "synth")]
    UGen(Arc<SynthDef>),
    #[cfg(feature = "faust")]
    Faust(Arc<FaustDef>),
}

// With neither def family compiled in, `NodeDef` is an empty enum: no node is
// ever built and each match reduces to the diverging `match *self {}` arm.
impl NodeDef {
    #[cfg_attr(
        not(any(feature = "synth", feature = "faust")),
        allow(unused_variables)
    )]
    pub fn control_index(&self, name: &str) -> Option<u32> {
        match self {
            #[cfg(feature = "synth")]
            NodeDef::UGen(def) => def.control_index(name),
            #[cfg(feature = "faust")]
            NodeDef::Faust(def) => def.control_index(name),
            #[cfg(not(any(feature = "synth", feature = "faust")))]
            _ => match *self {},
        }
    }

    /// Control name by index, for `/g_queryTree.reply`.
    #[cfg_attr(
        not(any(feature = "synth", feature = "faust")),
        allow(unused_variables)
    )]
    pub fn control_name(&self, index: usize) -> Option<&str> {
        match self {
            #[cfg(feature = "synth")]
            NodeDef::UGen(def) => def.control_names.get(index).map(String::as_str),
            #[cfg(feature = "faust")]
            NodeDef::Faust(def) => match index.checked_sub(def.params.len()) {
                None => def.params.get(index).map(|p| p.name.as_str()),
                Some(0) => Some("out"),
                Some(1) => Some("in"),
                Some(_) => None,
            },
            #[cfg(not(any(feature = "synth", feature = "faust")))]
            _ => match *self {},
        }
    }

    /// Number of addressable UGens (`/u_cmd`), or `None` for defs with no UGen
    /// vector (a Faust synth is one opaque block, not a UGen graph).
    fn ugen_count(&self) -> Option<usize> {
        match self {
            #[cfg(feature = "synth")]
            NodeDef::UGen(def) => Some(def.ugens.len()),
            #[cfg(feature = "faust")]
            NodeDef::Faust(_) => None,
            #[cfg(not(any(feature = "synth", feature = "faust")))]
            _ => match *self {},
        }
    }

    /// Default control values of a fresh instance.
    fn control_defaults(&self) -> Vec<f32> {
        match self {
            #[cfg(feature = "synth")]
            NodeDef::UGen(def) => def.control_defaults.clone(),
            #[cfg(feature = "faust")]
            NodeDef::Faust(def) => {
                // UI params at their inits, then the reserved out/in buses.
                let mut v: Vec<f32> = def.params.iter().map(|p| p.init).collect();
                v.extend([0.0, 0.0]);
                v
            }
            #[cfg(not(any(feature = "synth", feature = "faust")))]
            _ => match *self {},
        }
    }

    /// Bus usage of an instance with these control values (M12).
    #[cfg_attr(
        not(any(feature = "synth", feature = "faust")),
        allow(unused_variables)
    )]
    fn usage(&self, controls: &[f32]) -> (BusUsage, Vec<u32>) {
        match self {
            #[cfg(feature = "synth")]
            NodeDef::UGen(def) => ugen_usage(def, controls),
            #[cfg(feature = "faust")]
            NodeDef::Faust(def) => faust_usage(def, controls),
            #[cfg(not(any(feature = "synth", feature = "faust")))]
            _ => match *self {},
        }
    }
}

pub struct CmdTranslator {
    /// Faust instances bake the sample rate in at `/s_new` time.
    #[cfg_attr(not(feature = "faust"), allow(dead_code))]
    sample_rate: f32,
    /// Loaded SynthDefs; starts with the built-in "default".
    #[cfg(feature = "synth")]
    pub synth_defs: HashMap<String, Arc<SynthDef>>,
    /// Mirror of which def each live node was built from. Maintained from
    /// `/s_new` and from collected garbage (see [`CmdTranslator::forget_node`]).
    pub node_defs: HashMap<i32, NodeDef>,
    next_auto_id: i32,
    /// Compiled Faust defs by name, refcounted (every instance holds a clone).
    #[cfg(feature = "faust")]
    pub faust_defs: HashMap<String, Arc<FaustDef>>,
    /// Network-side tree mirror: topology, per-node controls and bus usage,
    /// auto-sorted groups (M12). Fed by the same commands the engine gets.
    pub mirror: TreeMirror,
    /// Mirror of the engine's buffer pool, kept in step with `/b_*` results
    /// (installed by the server). Read when building a Faust instance so its
    /// `soundfile` zones can be filled from a server buffer.
    pub buffers: BufferPool,
    /// M17: per-channel MIDI bindings and the live voice table. Channel-voice
    /// messages actuate nodes through [`Self::translate_midi`].
    pub midi: MidiBindings,
    /// M18: loaded GraphDefs by name, the live instances by their group id,
    /// and the private-bus allocators they draw from.
    pub graph_defs: HashMap<String, Arc<GraphDefSpec>>,
    pub graph_instances: HashMap<i32, GraphInstance>,
    /// Per-voice sub-graphs spawned by `/graph_voice` (or MIDI notes), keyed by
    /// their sub-group id.
    pub graph_voices: HashMap<i32, GraphVoice>,
    graph_audio_buses: RangeAllocator,
    graph_control_buses: RangeAllocator,
    /// Boot-time pool capacities. `max_group_children` sizes every non-root
    /// group this translator builds (`/g_new`, `/s_new`'s graph subgroups);
    /// `max_ugen_inputs` caps accepted inputs when compiling a def; the buffer
    /// pool `buffers` is already sized to `max_buffers` (its `len()` is the
    /// buffer-index bound). Kept so `/server_info` can report them.
    limits: Limits,
}

impl CmdTranslator {
    /// Translator with the default bus counts and pool limits (used by the NRT
    /// renderer and tests). The live server passes its configured counts via
    /// [`with_limits`](Self::with_limits).
    pub fn new(sample_rate: f32) -> Self {
        Self::with_buses(sample_rate, NUM_AUDIO_BUSES, NUM_CONTROL_BUSES)
    }

    /// Configured bus counts with default pool limits.
    pub fn with_buses(sample_rate: f32, audio_buses: usize, control_buses: usize) -> Self {
        Self::with_limits(sample_rate, audio_buses, control_buses, Limits::default())
    }

    /// Fully configured: bus counts plus the boot-time pool [`Limits`].
    pub fn with_limits(
        sample_rate: f32,
        audio_buses: usize,
        control_buses: usize,
        limits: Limits,
    ) -> Self {
        let limits = limits.clamped();
        // Reserve the top of each bus space for GraphDef private buses, shrinking
        // the reservation if the configured count is smaller than the default.
        let audio_reserved = GRAPH_AUDIO_BUS_RESERVED.min(audio_buses);
        let control_reserved = GRAPH_CONTROL_BUS_RESERVED.min(control_buses);
        #[cfg(feature = "synth")]
        let synth_defs = {
            let mut synth_defs = HashMap::new();
            let default = compile(default_spec()).expect("built-in default def must compile");
            synth_defs.insert(default.name.clone(), Arc::new(default));
            synth_defs
        };
        Self {
            sample_rate,
            #[cfg(feature = "synth")]
            synth_defs,
            node_defs: HashMap::new(),
            next_auto_id: AUTO_NODE_ID_BASE,
            #[cfg(feature = "faust")]
            faust_defs: HashMap::new(),
            mirror: TreeMirror::new(),
            buffers: empty_pool_with(limits.max_buffers),
            midi: MidiBindings::new(),
            graph_defs: HashMap::new(),
            graph_instances: HashMap::new(),
            graph_voices: HashMap::new(),
            graph_audio_buses: RangeAllocator::new(audio_buses - audio_reserved, audio_reserved),
            graph_control_buses: RangeAllocator::new(
                control_buses - control_reserved,
                control_reserved,
            ),
            limits,
        }
    }

    /// A non-root group sized to the configured `--max-graph-children`.
    fn new_group(&self) -> Group {
        Group::with_capacity(self.limits.max_group_children)
    }

    /// Total defs of both families, for `/status.reply`.
    pub fn def_count(&self) -> usize {
        #[allow(unused_mut)]
        let mut n = 0;
        #[cfg(feature = "synth")]
        {
            n += self.synth_defs.len();
        }
        #[cfg(feature = "faust")]
        {
            n += self.faust_defs.len();
        }
        n
    }

    /// Builds a synth instance from either def table. Faust instantiation
    /// (`createCDSPInstance` + `init`) allocates — fine, this never runs on
    /// the audio thread; the boxed instance reaches it fully built.
    pub fn make_synth(&self, name: &str) -> Result<(Box<dyn SynthNode>, NodeDef), String> {
        #[cfg(feature = "synth")]
        if let Some(def) = self.synth_defs.get(name) {
            let synth = Box::new(UGenSynth::new(Arc::clone(def)));
            return Ok((synth, NodeDef::UGen(Arc::clone(def))));
        }
        #[cfg(feature = "faust")]
        if let Some(def) = self.faust_defs.get(name) {
            let synth = FaustSynth::new(Arc::clone(def), self.sample_rate, &self.buffers)?;
            return Ok((Box::new(synth), NodeDef::Faust(Arc::clone(def))));
        }
        Err(format!("SynthDef not found: {name}"))
    }

    /// Drops the mirror entries of a node the engine freed or rejected.
    pub fn forget_node(&mut self, id: i32) {
        self.node_defs.remove(&id);
        self.mirror.remove(id);
    }

    /// Re-sorts every auto group on the ancestor chain starting at `group`,
    /// appending the move commands (and updating the mirror). Removals never
    /// invalidate a topological order, so callers skip this on frees.
    fn resort_from(&mut self, group: Option<i32>, cmds: &mut Vec<Cmd>) {
        let Some(mut group) = group else { return };
        loop {
            if self.mirror.is_auto_group(group)
                && let Some(order) = self.mirror.sorted_children(group)
            {
                for pair in order.windows(2) {
                    cmds.push(Cmd::MoveNode {
                        id: pair[1],
                        target: pair[0],
                        place: Place::After,
                    });
                }
                self.mirror.set_children_order(group, order);
            }
            match self.mirror.parent(group) {
                Some(parent) => group = parent,
                None => break,
            }
        }
    }

    /// Re-analyzes a synth's bus usage after its controls changed.
    fn refresh_usage(&mut self, id: i32) {
        let Some(def) = self.node_defs.get(&id) else {
            return;
        };
        let Some((_, controls)) = self.mirror.synth_info(id) else {
            return;
        };
        let (base, _) = def.usage(controls);
        let usage = self.mirror.fold_maps_into_usage(id, base);
        self.mirror.set_usage(id, usage);
    }

    /// After a control change altered a synth's effective bus usage:
    /// re-analyze, ship the fresh masks to the engine (the M13 scheduler keeps
    /// its own copy), and re-sort the parent auto group.
    fn reanalyze_and_resort(&mut self, id: i32, cmds: &mut Vec<Cmd>) {
        self.refresh_usage(id);
        if self.mirror.synth_info(id).is_some() {
            let usage = self.mirror.usage_of(id);
            cmds.push(Cmd::SetUsage { id, usage });
        }
        self.resort_from(self.mirror.parent(id), cmds);
    }

    /// The synth nodes a `/n_set`/`/n_map`/`/n_mapa` targets. A synth targets
    /// itself; a **group** propagates the named controls to every synth/faust
    /// in its subtree, recursing through subgroups and stopping at each synth
    /// — scsynth's group semantics, "transfer the named parameters down to the
    /// subgroups until a synth/faust def is reached". A node whose name has no
    /// matching control is simply skipped (its `control_key` is `None`).
    /// Unknown ids yield an empty list, so the caller can `/fail`.
    ///
    /// A GraphDef instance group is intercepted *before* this (its named
    /// surface, not raw member propagation — see [`Self::graph_set`]), so it
    /// never reaches here.
    fn control_targets(&self, id: i32) -> Vec<i32> {
        match self.mirror.get(id).map(|n| &n.body) {
            Some(MirrorBody::Synth { .. }) => vec![id],
            Some(MirrorBody::Group { .. }) => {
                let mut out = Vec::new();
                self.collect_subtree_synths(id, &mut out);
                out
            }
            // Not mirrored but a def we know (defensive: a synth whose mirror
            // insert was rejected) still sets itself.
            None if self.node_defs.contains_key(&id) => vec![id],
            None => Vec::new(),
        }
    }

    fn collect_subtree_synths(&self, group: i32, out: &mut Vec<i32>) {
        let Some(children) = self.mirror.children(group) else {
            return;
        };
        // Snapshot so the immutable borrow is released before recursing.
        for child in children.to_vec() {
            match self.mirror.get(child).map(|n| &n.body) {
                Some(MirrorBody::Synth { .. }) => out.push(child),
                Some(MirrorBody::Group { .. }) => self.collect_subtree_synths(child, out),
                None => {}
            }
        }
    }

    /// True iff `id` is unknown (neither in the tree mirror nor a node whose
    /// def we still hold), so a `/n_set`/`/n_map` on it should `/fail`. An
    /// empty group is *known* — propagation is just a no-op.
    fn node_unknown(&self, id: i32) -> bool {
        self.mirror.get(id).is_none() && !self.node_defs.contains_key(&id)
    }

    /// `/n_map` (control bus) and `/n_mapa` (audio bus): bind controls to
    /// buses the synth reads at the start of every block, `bus = -1` to
    /// unbind. Same pair-wise parsing as `/n_set`; an audio map (or mapping a
    /// control used as a bus index) re-analyzes the node's bus usage. Like
    /// `/n_set`, a group target propagates the maps over its subtree.
    fn map_controls(
        &mut self,
        msg: &rosc::OscMessage,
        audio: bool,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let Some(OscType::Int(id)) = msg.args.first() else {
            return Err("expected: id, then control/bus pairs".into());
        };
        if self.node_unknown(*id) {
            return Err(format!("node {id} not found"));
        }
        for node in self.control_targets(*id) {
            let Some(def) = self.node_defs.get(&node).cloned() else {
                continue;
            };
            let mut usage_hit = false;
            for pair in msg.args[1..].chunks(2) {
                if let (Some(index), Some(bus)) =
                    (control_key(&pair[0], &def), pair.get(1).and_then(int_value))
                {
                    cmds.push(Cmd::MapControl {
                        id: node,
                        index,
                        bus,
                        audio,
                    });
                    usage_hit |= self.mirror.set_map(node, index, bus, audio);
                }
            }
            if usage_hit {
                self.reanalyze_and_resort(node, cmds);
            }
        }
        Ok(())
    }

    /// `/n_setn nodeID [ctrl numControls val...]...`: like `/n_set`, but each
    /// group sets a **consecutive range** of controls starting at `ctrl`
    /// (resolved by name or index). A group target propagates over its subtree.
    fn set_controls_n(
        &mut self,
        msg: &rosc::OscMessage,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let Some(OscType::Int(id)) = msg.args.first() else {
            return Err("expected: id, then (control, numControls, values...) groups".into());
        };
        if self.node_unknown(*id) {
            return Err(format!("node {id} not found"));
        }
        for node in self.control_targets(*id) {
            let Some(def) = self.node_defs.get(&node).cloned() else {
                continue;
            };
            let mut bus_control_hit = false;
            let mut rest = &msg.args[1..];
            while !rest.is_empty() {
                let [ctrl, OscType::Int(count), tail @ ..] = rest else {
                    return Err("expected (control, numControls, values...) groups".into());
                };
                let count = usize::try_from(*count).map_err(|_| "numControls must be >= 0")?;
                if tail.len() < count {
                    return Err("fewer values than numControls".into());
                }
                let base = control_key(ctrl, &def).ok_or("unknown control")?;
                for (offset, value) in tail[..count].iter().enumerate() {
                    let value = float_value(value).ok_or("expected number values")?;
                    let index = base + offset as u32;
                    cmds.push(Cmd::SetControl {
                        id: node,
                        index,
                        value,
                    });
                    bus_control_hit |= self.mirror.set_control(node, index, value);
                    bus_control_hit |= self.mirror.set_map(node, index, -1, false);
                }
                rest = &tail[count..];
            }
            if bus_control_hit {
                self.reanalyze_and_resort(node, cmds);
            }
        }
        Ok(())
    }

    /// `/n_fill nodeID [ctrl numControls value]...`: fills a consecutive range
    /// of controls with a single value (each group a `(ctrl, numControls,
    /// value)` triple). Propagates over a group target like `/n_set`.
    fn fill_controls(&mut self, msg: &rosc::OscMessage, cmds: &mut Vec<Cmd>) -> Result<(), String> {
        let Some(OscType::Int(id)) = msg.args.first() else {
            return Err("expected: id, then (control, numControls, value) triples".into());
        };
        if self.node_unknown(*id) {
            return Err(format!("node {id} not found"));
        }
        if !msg.args[1..].len().is_multiple_of(3) {
            return Err("expected (control, numControls, value) triples".into());
        }
        for node in self.control_targets(*id) {
            let Some(def) = self.node_defs.get(&node).cloned() else {
                continue;
            };
            let mut bus_control_hit = false;
            for group in msg.args[1..].chunks(3) {
                let [ctrl, OscType::Int(count), val] = group else {
                    return Err("expected (control, numControls, value) triples".into());
                };
                let count = u32::try_from(*count).map_err(|_| "numControls must be >= 0")?;
                let value = float_value(val).ok_or("expected number value")?;
                let base = control_key(ctrl, &def).ok_or("unknown control")?;
                for offset in 0..count {
                    let index = base + offset;
                    cmds.push(Cmd::SetControl {
                        id: node,
                        index,
                        value,
                    });
                    bus_control_hit |= self.mirror.set_control(node, index, value);
                    bus_control_hit |= self.mirror.set_map(node, index, -1, false);
                }
            }
            if bus_control_hit {
                self.reanalyze_and_resort(node, cmds);
            }
        }
        Ok(())
    }

    /// `/n_mapn` / `/n_mapan`: like `/n_map`/`/n_mapa`, but each group
    /// `(ctrl, busIndex, numControls)` maps `numControls` **consecutive**
    /// controls to `numControls` **consecutive** buses starting at `busIndex`
    /// (`busIndex = -1` unbinds the whole range).
    fn map_controls_n(
        &mut self,
        msg: &rosc::OscMessage,
        audio: bool,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let Some(OscType::Int(id)) = msg.args.first() else {
            return Err("expected: id, then (control, busIndex, numControls) groups".into());
        };
        if self.node_unknown(*id) {
            return Err(format!("node {id} not found"));
        }
        if !msg.args[1..].len().is_multiple_of(3) {
            return Err("expected (control, busIndex, numControls) groups".into());
        }
        for node in self.control_targets(*id) {
            let Some(def) = self.node_defs.get(&node).cloned() else {
                continue;
            };
            let mut usage_hit = false;
            for group in msg.args[1..].chunks(3) {
                let [ctrl, OscType::Int(bus), OscType::Int(count)] = group else {
                    return Err("expected int busIndex and numControls".into());
                };
                let count = u32::try_from(*count).map_err(|_| "numControls must be >= 0")?;
                let base = control_key(ctrl, &def).ok_or("unknown control")?;
                for offset in 0..count {
                    let index = base + offset;
                    // -1 unbinds every control in the range; else buses advance.
                    let bus = if *bus < 0 { -1 } else { *bus + offset as i32 };
                    cmds.push(Cmd::MapControl {
                        id: node,
                        index,
                        bus,
                        audio,
                    });
                    usage_hit |= self.mirror.set_map(node, index, bus, audio);
                }
            }
            if usage_hit {
                self.reanalyze_and_resort(node, cmds);
            }
        }
        Ok(())
    }

    /// `/n_order addAction targetID nodeID...`: moves several nodes to one
    /// position in listed order. `addAction` 0 = head of the target group, 1 =
    /// tail, 2 = before the target node, 3 = after it. The first node goes to
    /// the position; each following node lands right after the previous one, so
    /// they keep the given order. Auto-sorted groups (`/g_sortMode`) reject
    /// manual moves, same as `/n_before`.
    fn order_nodes(&mut self, msg: &rosc::OscMessage, cmds: &mut Vec<Cmd>) -> Result<(), String> {
        let [OscType::Int(action), OscType::Int(target), nodes @ ..] = msg.args.as_slice() else {
            return Err("expected: addAction, targetID, then node IDs".into());
        };
        // The first move is relative to the target; the rest chain after the
        // previous node so the list order is preserved.
        let (mut place, mut anchor) = match action {
            0 => (Place::Head, *target),
            1 => (Place::Tail, *target),
            2 => (Place::Before, *target),
            3 => (Place::After, *target),
            _ => {
                return Err("addAction must be 0 (head), 1 (tail), 2 (before) or 3 (after)".into());
            }
        };
        for node in nodes {
            let OscType::Int(id) = node else {
                return Err("expected int node IDs".into());
            };
            self.move_one(*id, anchor, place, cmds)?;
            place = Place::After;
            anchor = *id;
        }
        Ok(())
    }

    /// `/g_head` / `/g_tail groupID nodeID...`: moves each node to the head/tail
    /// of the given group (pairs of `groupID, nodeID`).
    fn move_to_group(&mut self, msg: &rosc::OscMessage, cmds: &mut Vec<Cmd>) -> Result<(), String> {
        let place = if msg.addr == "/g_head" {
            Place::Head
        } else {
            Place::Tail
        };
        if msg.args.is_empty() || !msg.args.len().is_multiple_of(2) {
            return Err("expected (groupID, nodeID) pairs".into());
        }
        for pair in msg.args.chunks(2) {
            let [OscType::Int(group), OscType::Int(id)] = pair else {
                return Err("expected int (groupID, nodeID) pairs".into());
            };
            self.move_one(*id, *group, place, cmds)?;
        }
        Ok(())
    }

    /// One node move shared by `/n_order`, `/g_head` and `/g_tail`: rejects
    /// moving into an auto-sorted group, emits the `Cmd::MoveNode`, and re-sorts
    /// the affected auto ancestors. `target` is a sibling (Before/After) or the
    /// destination group (Head/Tail).
    fn move_one(
        &mut self,
        id: i32,
        target: i32,
        place: Place,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        // The destination group is the target itself (Head/Tail) or the
        // target's parent (Before/After); manual moves into it are the auto
        // group's job (M12).
        let dest = match place {
            Place::Head | Place::Tail => target,
            Place::Before | Place::After => self.mirror.parent(target).unwrap_or(target),
        };
        if self.mirror.is_auto_group(dest) {
            return Err(format!(
                "group {dest} is auto-sorted (/g_sortMode): manual moves are disabled there"
            ));
        }
        cmds.push(Cmd::MoveNode { id, target, place });
        if let Some((old_parent, new_parent)) = self.mirror.move_node(id, target, place) {
            self.resort_from(Some(old_parent), cmds);
            if new_parent != old_parent {
                self.resort_from(Some(new_parent), cmds);
            }
        }
        Ok(())
    }

    /// `/c_setn busIndex numBuses val...`: sets a consecutive range of control
    /// buses (one or more `(busIndex, numBuses, values...)` groups). The
    /// **immediate** form writes the shared atomics on the network thread; the
    /// scheduled form (this) ships `Cmd::SetControlBus` per bus.
    fn set_control_bus_n(
        &mut self,
        msg: &rosc::OscMessage,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let mut rest = msg.args.as_slice();
        while !rest.is_empty() {
            let [OscType::Int(base), OscType::Int(count), tail @ ..] = rest else {
                return Err("expected (busIndex, numBuses, values...) groups".into());
            };
            if *base < 0 {
                return Err("bus index must be non-negative".into());
            }
            let count = usize::try_from(*count).map_err(|_| "numBuses must be >= 0")?;
            if tail.len() < count {
                return Err("fewer values than numBuses".into());
            }
            for (offset, value) in tail[..count].iter().enumerate() {
                let value = float_value(value).ok_or("expected number values")?;
                cmds.push(Cmd::SetControlBus {
                    index: *base as usize + offset,
                    value,
                });
            }
            rest = &tail[count..];
        }
        Ok(())
    }

    /// `/c_fill busIndex numBuses value...`: fills a consecutive range of
    /// control buses with one value (groups of `(busIndex, numBuses, value)`).
    fn fill_control_bus(
        &mut self,
        msg: &rosc::OscMessage,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        if msg.args.is_empty() || !msg.args.len().is_multiple_of(3) {
            return Err("expected (busIndex, numBuses, value) triples".into());
        }
        for group in msg.args.chunks(3) {
            let [OscType::Int(base), OscType::Int(count), val] = group else {
                return Err("expected int busIndex and numBuses".into());
            };
            if *base < 0 {
                return Err("bus index must be non-negative".into());
            }
            let count = usize::try_from(*count).map_err(|_| "numBuses must be >= 0")?;
            let value = float_value(val).ok_or("expected number value")?;
            for offset in 0..count {
                cmds.push(Cmd::SetControlBus {
                    index: *base as usize + offset,
                    value,
                });
            }
        }
        Ok(())
    }

    /// `/u_cmd nodeID ugenIndex commandName args...`: a typed command addressed
    /// to one UGen instance — the discoverable replacement for scsynth's
    /// untyped `/u_cmd`. The command name is hashed to a stable selector and
    /// the numeric args are packed inline (no heap crosses to the audio
    /// thread). Validates the node is a UGen synth and the index is in range;
    /// the specific commands a UGen understands land with that UGen.
    fn ugen_command(&mut self, msg: &rosc::OscMessage, cmds: &mut Vec<Cmd>) -> Result<(), String> {
        let [
            OscType::Int(id),
            OscType::Int(ugen_index),
            OscType::String(name),
            rest @ ..,
        ] = msg.args.as_slice()
        else {
            return Err("expected: nodeID, ugenIndex, commandName, args...".into());
        };
        let Some(def) = self.node_defs.get(id) else {
            return Err(format!("synth {id} not found"));
        };
        let Some(count) = def.ugen_count() else {
            return Err(format!("node {id} is not a UGen synth"));
        };
        let ugen_index = u32::try_from(*ugen_index).map_err(|_| "ugenIndex must be >= 0")?;
        if ugen_index as usize >= count {
            return Err(format!(
                "ugenIndex {ugen_index} out of range (synth has {count})"
            ));
        }
        if rest.len() > MAX_UGEN_CMD_ARGS {
            return Err(format!("at most {MAX_UGEN_CMD_ARGS} command args"));
        }
        let mut args = [0.0; MAX_UGEN_CMD_ARGS];
        for (slot, arg) in args.iter_mut().zip(rest) {
            *slot = float_value(arg).ok_or("expected number command args")?;
        }
        cmds.push(Cmd::UGenCommand {
            id: *id,
            ugen_index,
            command: UGenCmd {
                selector: ugen_cmd_selector(name),
                args,
                num_args: rest.len() as u8,
            },
        });
        Ok(())
    }

    /// `/d_recv`: compile a SynthDef JSON blob into the def table. Returns the
    /// def name, so the caller can persist the spec under it.
    #[cfg(feature = "synth")]
    pub fn d_recv(&mut self, args: &[OscType]) -> Result<String, String> {
        let bytes: &[u8] = match args.first() {
            Some(OscType::Blob(b)) => b,
            Some(OscType::String(s)) => s.as_bytes(),
            _ => return Err("expected a JSON blob or string".into()),
        };
        let spec: SynthDefSpec =
            serde_json::from_slice(bytes).map_err(|e| format!("invalid JSON: {e}"))?;
        let def = compile(spec)?;
        // `compile` already enforced the hard ceiling; reject anything past the
        // stricter boot-time `--max-ugen-inputs` too (default: the ceiling).
        if let Some((i, u)) = def
            .ugens
            .iter()
            .enumerate()
            .find(|(_, u)| u.inputs.len() > self.limits.max_ugen_inputs)
        {
            return Err(format!(
                "ugens[{i}]: inputs ({}) exceed --max-ugen-inputs ({})",
                u.inputs.len(),
                self.limits.max_ugen_inputs
            ));
        }
        let name = def.name.clone();
        self.synth_defs.insert(name.clone(), Arc::new(def));
        Ok(name)
    }

    /// `/d_recv` on a server built without the SynthDef family.
    #[cfg(not(feature = "synth"))]
    pub fn d_recv(&mut self, _args: &[OscType]) -> Result<String, String> {
        Err("server built without synthdef support".into())
    }

    /// `/d_free name...`. Live synths keep their `Arc<SynthDef>`: scsynth
    /// semantics. Same for Faust factories (instances refcount them).
    pub fn d_free(&mut self, args: &[OscType]) -> Result<(), String> {
        for arg in args {
            let OscType::String(name) = arg else {
                return Err("expected synthdef names".into());
            };
            #[cfg(feature = "synth")]
            self.synth_defs.remove(name);
            #[cfg(feature = "faust")]
            self.faust_defs.remove(name);
            self.graph_defs.remove(name);
        }
        Ok(())
    }

    /// `/d_graph <json>`: parse and validate a GraphDef spec, store it under
    /// its name. Returns the name so the caller can persist it. Cheap (no
    /// JIT): a GraphDef only references other defs, each carrying its own
    /// compile/cache.
    pub fn d_graph(&mut self, args: &[OscType]) -> Result<String, String> {
        let bytes: &[u8] = match args.first() {
            Some(OscType::Blob(b)) => b,
            Some(OscType::String(s)) => s.as_bytes(),
            _ => return Err("expected a JSON blob or string".into()),
        };
        let spec: GraphDefSpec =
            serde_json::from_slice(bytes).map_err(|e| format!("invalid JSON: {e}"))?;
        spec.validate()?;
        let name = spec.name.clone();
        self.graph_defs.insert(name.clone(), Arc::new(spec));
        Ok(name)
    }

    /// Drops a GraphDef. Live instances keep running (they hold no reference
    /// to the def). Folded into `/d_free` alongside the synth/faust tables.
    pub fn graph_def_free(&mut self, name: &str) {
        self.graph_defs.remove(name);
    }

    /// Allocates a GraphDef's private buses (resolved name → first index).
    /// On a shortfall it hands back everything it took, so the caller's later
    /// steps stay side-effect-free until this succeeds.
    fn alloc_graph_buses(
        &mut self,
        def: &GraphDefSpec,
    ) -> Result<
        (
            HashMap<String, usize>,
            Vec<(usize, usize)>,
            Vec<(usize, usize)>,
        ),
        String,
    > {
        let mut bus_index = HashMap::new();
        let mut audio: Vec<(usize, usize)> = Vec::new();
        let mut control: Vec<(usize, usize)> = Vec::new();
        for b in &def.buses {
            let width = b.channels.max(1);
            let first = match b.rate {
                BusRate::Audio => self.graph_audio_buses.alloc(width),
                BusRate::Control => self.graph_control_buses.alloc(width),
            };
            let Some(first) = first else {
                for (f, w) in audio {
                    self.graph_audio_buses.free(f, w);
                }
                for (f, w) in control {
                    self.graph_control_buses.free(f, w);
                }
                return Err("out of private buses for GraphDef".into());
            };
            match b.rate {
                BusRate::Audio => audio.push((first, width)),
                BusRate::Control => control.push((first, width)),
            }
            bus_index.insert(b.name.clone(), first);
        }
        Ok((bus_index, audio, control))
    }

    /// Instantiates the members at `indices` inside `parent`, consuming the
    /// pre-built synths (parallel to `indices`): sets each control (bus
    /// references resolved against `bus_index`, `"OUT"` → bus 0) and applies
    /// the `/n_map` wiring. Returns member index → node id. Infallible — the
    /// fallible `make_synth` happened in the caller, so an instance is never
    /// left half-built.
    fn build_members(
        &mut self,
        def: &GraphDefSpec,
        indices: &[usize],
        built: Vec<(Box<dyn SynthNode>, NodeDef)>,
        parent: i32,
        bus_index: &HashMap<String, usize>,
        cmds: &mut Vec<Cmd>,
    ) -> HashMap<usize, i32> {
        let mut node_of: HashMap<usize, i32> = HashMap::new();
        for (&mi, (mut synth, ndef)) in indices.iter().zip(built) {
            let member = &def.members[mi];
            self.next_auto_id += 1;
            let node_id = self.next_auto_id;
            let mut controls = ndef.control_defaults();
            for (cname, cval) in &member.controls {
                let Some(index) = ndef.control_index(cname) else {
                    continue;
                };
                let value = match cval {
                    ControlValue::Num(v) => *v,
                    ControlValue::Bus(b) if b == "OUT" => 0.0,
                    // validate() guaranteed the name resolves.
                    ControlValue::Bus(b) => bus_index[b.as_str()] as f32,
                };
                synth.set_control(index, value);
                if let Some(slot) = controls.get_mut(index as usize) {
                    *slot = value;
                }
            }
            let (usage, bus_controls) = ndef.usage(&controls);
            self.node_defs.insert(node_id, ndef);
            cmds.push(Cmd::AddSynth {
                id: node_id,
                target: parent,
                action: AddAction::Tail,
                synth,
                usage,
            });
            let _ = self.mirror.insert(
                node_id,
                MirrorBody::Synth {
                    def_name: member.def.clone(),
                    controls,
                    usage,
                    bus_controls,
                    maps: Vec::new(),
                },
                parent,
                AddAction::Tail,
            );
            node_of.insert(mi, node_id);
        }
        // `/n_map` wiring, once every member exists.
        for &mi in indices {
            let node_id = node_of[&mi];
            for (cname, bname) in &def.members[mi].maps {
                let Some(index) = self
                    .node_defs
                    .get(&node_id)
                    .and_then(|d| d.control_index(cname))
                else {
                    continue;
                };
                let bus = bus_index[bname.as_str()] as i32;
                cmds.push(Cmd::MapControl {
                    id: node_id,
                    index,
                    bus,
                    audio: false,
                });
                self.mirror.set_map(node_id, index, bus, false);
            }
        }
        node_of
    }

    /// Resolves the surface ports whose targets are *all* present in
    /// `node_of` → `(node id, control index, mul, add)`. So passing the shared
    /// member map yields the shared ports and passing a voice's member map
    /// yields the voice ports (a port never mixes the two — see `validate`).
    fn resolve_ports(&self, def: &GraphDefSpec, node_of: &HashMap<usize, i32>) -> ResolvedSurface {
        let mut surface = ResolvedSurface::new();
        for (port, targets) in &def.surface {
            if !targets.iter().all(|t| node_of.contains_key(&t.member)) {
                continue;
            }
            let resolved = targets
                .iter()
                .filter_map(|t| {
                    let node = node_of[&t.member];
                    self.node_defs
                        .get(&node)
                        .and_then(|d| d.control_index(&t.control))
                        .map(|index| (node, index, t.mul, t.add))
                })
                .collect();
            surface.insert(port.clone(), resolved);
        }
        surface
    }

    /// Collects the per-instantiation `port value` overrides trailing a
    /// `/graph_new`/`/graph_voice`.
    fn port_overrides(rest: &[OscType]) -> Vec<(String, f32)> {
        rest.chunks(2)
            .filter_map(
                |pair| match (pair.first(), pair.get(1).and_then(float_value)) {
                    (Some(OscType::String(port)), Some(value)) => Some((port.clone(), value)),
                    _ => None,
                },
            )
            .collect()
    }

    /// `/graph_new name id action target [port value ...]`: instantiate a
    /// GraphDef as a group holding its **shared** members, with private buses
    /// and a resolved named surface. (Per-voice members wait for
    /// `/graph_voice`.) It expands entirely into existing primitives (a group,
    /// member `/s_new`s, `/n_map` wiring), so the engine sees nothing new and
    /// RT-safety is untouched. Atomic: every fallible step (member def
    /// resolution, bus allocation) happens before any command or mirror change.
    fn graph_new(&mut self, msg: &rosc::OscMessage, cmds: &mut Vec<Cmd>) -> Result<(), String> {
        let [
            OscType::String(name),
            OscType::Int(id),
            OscType::Int(action),
            OscType::Int(target),
            rest @ ..,
        ] = msg.args.as_slice()
        else {
            return Err("expected: name, id, addAction, targetID [, port, value ...]".into());
        };
        let def = self
            .graph_defs
            .get(name)
            .cloned()
            .ok_or_else(|| format!("GraphDef not found: {name}"))?;
        let action = AddAction::from_i32(*action).ok_or("add action must be 0-4")?;
        let group_id = match *id {
            -1 => {
                self.next_auto_id += 1;
                self.next_auto_id
            }
            n if n > 0 => n,
            _ => return Err("group ID must be positive or -1".into()),
        };

        // --- fallible phase: nothing observable happens until it all passes.
        let shared: Vec<usize> = def
            .members
            .iter()
            .enumerate()
            .filter(|(_, m)| !m.voice)
            .map(|(i, _)| i)
            .collect();
        let mut built = Vec::with_capacity(shared.len());
        for &mi in &shared {
            built.push(self.make_synth(&def.members[mi].def)?);
        }
        let (bus_index, audio_buses, control_buses) = self.alloc_graph_buses(&def)?;

        // --- infallible phase: build the instance. The instance group is
        // auto-sorted so member (and voice sub-group) order follows the bus
        // connections (M12); manual ordering is the graph's, not the client's.
        cmds.push(Cmd::AddGroup {
            id: group_id,
            target: *target,
            action,
            group: self.new_group(),
        });
        let _ = self.mirror.insert(
            group_id,
            MirrorBody::Group {
                children: Vec::new(),
                auto: true,
                parallel: false,
            },
            *target,
            action,
        );
        let shared_nodes = self.build_members(&def, &shared, built, group_id, &bus_index, cmds);
        self.resort_from(Some(group_id), cmds);
        let surface = self.resolve_ports(&def, &shared_nodes);
        self.graph_instances.insert(
            group_id,
            GraphInstance {
                def: Arc::clone(&def),
                shared_nodes,
                bus_index,
                audio_buses,
                control_buses,
                surface,
                voices: HashSet::new(),
            },
        );

        // Shared-port defaults, then the per-instantiation overrides.
        let mut ports: Vec<(String, f32)> = def
            .defaults
            .iter()
            .filter(|(p, _)| !def.is_voice_port(p))
            .map(|(p, v)| (p.clone(), *v))
            .collect();
        ports.extend(Self::port_overrides(rest));
        for (port, value) in ports {
            self.apply_surface(group_id, &port, value, cmds);
        }
        Ok(())
    }

    /// `/graph_voice instanceID id [port value ...]`: spawn a per-voice
    /// sub-graph inside a running GraphDef instance, wired to its shared
    /// private buses. The voice is a sub-group at the head of the instance
    /// group (the auto-sort then orders it relative to the shared mixer by its
    /// bus usage); freeing it (`/n_free`) frees its members. Same atomic
    /// shape as `/graph_new`.
    fn graph_voice(&mut self, msg: &rosc::OscMessage, cmds: &mut Vec<Cmd>) -> Result<(), String> {
        let [OscType::Int(instance), OscType::Int(id), rest @ ..] = msg.args.as_slice() else {
            return Err("expected: instanceID, voiceID [, port, value ...]".into());
        };
        let Some(inst) = self.graph_instances.get(instance) else {
            return Err(format!("GraphDef instance {instance} not found"));
        };
        let def = Arc::clone(&inst.def);
        let bus_index = inst.bus_index.clone();
        if !def.has_voice_members() {
            return Err("GraphDef has no per-voice members".into());
        }
        let voice_indices: Vec<usize> = def
            .members
            .iter()
            .enumerate()
            .filter(|(_, m)| m.voice)
            .map(|(i, _)| i)
            .collect();
        let voice_id = match *id {
            -1 => {
                self.next_auto_id += 1;
                self.next_auto_id
            }
            n if n > 0 => n,
            _ => return Err("voice ID must be positive or -1".into()),
        };
        // fallible: build the voice's synths before touching anything.
        let mut built = Vec::with_capacity(voice_indices.len());
        for &mi in &voice_indices {
            built.push(self.make_synth(&def.members[mi].def)?);
        }
        // infallible: the voice sub-group, into the instance group.
        cmds.push(Cmd::AddGroup {
            id: voice_id,
            target: *instance,
            action: AddAction::Head,
            group: self.new_group(),
        });
        let _ = self.mirror.insert(
            voice_id,
            MirrorBody::Group {
                children: Vec::new(),
                auto: true,
                parallel: false,
            },
            *instance,
            AddAction::Head,
        );
        let voice_nodes =
            self.build_members(&def, &voice_indices, built, voice_id, &bus_index, cmds);
        // Resort the voice's own members and, up the chain, the instance group
        // (so the voice runs before the shared mixer that reads its bus).
        self.resort_from(Some(voice_id), cmds);
        let surface = self.resolve_ports(&def, &voice_nodes);
        self.graph_voices.insert(
            voice_id,
            GraphVoice {
                instance: *instance,
                surface,
            },
        );
        if let Some(inst) = self.graph_instances.get_mut(instance) {
            inst.voices.insert(voice_id);
        }

        // Voice-port defaults, then the per-spawn overrides.
        let mut ports: Vec<(String, f32)> = def
            .defaults
            .iter()
            .filter(|(p, _)| def.is_voice_port(p))
            .map(|(p, v)| (p.clone(), *v))
            .collect();
        ports.extend(Self::port_overrides(rest));
        for (port, value) in ports {
            self.apply_surface(voice_id, &port, value, cmds);
        }
        Ok(())
    }

    /// Writes a surface-port value to its resolved member controls, scaled per
    /// target (`mul`·v + `add`), mirroring each write and re-sorting if a
    /// target turns out to be a bus-index control. `group` may be an instance
    /// (shared surface) or a voice sub-group (voice surface).
    fn apply_surface(&mut self, group: i32, port: &str, value: f32, cmds: &mut Vec<Cmd>) {
        let targets = self
            .graph_instances
            .get(&group)
            .and_then(|inst| inst.surface.get(port))
            .or_else(|| {
                self.graph_voices
                    .get(&group)
                    .and_then(|v| v.surface.get(port))
            });
        let targets = match targets {
            Some(t) => t.clone(),
            None => return,
        };
        for (node, index, mul, add) in targets {
            let v = mul * value + add;
            cmds.push(Cmd::SetControl {
                id: node,
                index,
                value: v,
            });
            let mut hit = self.mirror.set_control(node, index, v);
            hit |= self.mirror.set_map(node, index, -1, false);
            if hit {
                self.reanalyze_and_resort(node, cmds);
            }
        }
    }

    /// If `id` is a GraphDef instance or a voice sub-group, apply each
    /// `(port, value)` pair against its named surface and return true. Names
    /// absent from the surface are ignored — the surface is the whole public
    /// interface; the member node ids stay private. Anything else returns
    /// false so `/n_set` falls back to the synth/group path.
    fn graph_set(&mut self, id: i32, pairs: &[OscType], cmds: &mut Vec<Cmd>) -> bool {
        if !self.graph_instances.contains_key(&id) && !self.graph_voices.contains_key(&id) {
            return false;
        }
        for pair in pairs.chunks(2) {
            if let (Some(OscType::String(port)), Some(value)) =
                (pair.first(), pair.get(1).and_then(float_value))
            {
                self.apply_surface(id, port, value, cmds);
            }
        }
        true
    }

    /// Drops the translator-side state of a freed GraphDef node: a voice
    /// sub-group (forget it, detach from its instance) or an instance group
    /// (reclaim its private buses and forget its voices). A no-op for ordinary
    /// nodes. The actual node teardown is the `/n_free` `FreeNode` itself.
    fn free_graph_node(&mut self, id: i32) {
        if let Some(voice) = self.graph_voices.remove(&id) {
            if let Some(inst) = self.graph_instances.get_mut(&voice.instance) {
                inst.voices.remove(&id);
            }
            return;
        }
        if let Some(inst) = self.graph_instances.remove(&id) {
            for v in inst.voices {
                self.graph_voices.remove(&v);
            }
            for (first, width) in inst.audio_buses {
                self.graph_audio_buses.free(first, width);
            }
            for (first, width) in inst.control_buses {
                self.graph_control_buses.free(first, width);
            }
        }
    }

    /// Translates one schedulable message into commands, appending to `cmds`.
    /// Everything allocating (boxed synths, name resolution) happens now;
    /// nothing reaches the engine until the caller ships the batch.
    pub fn translate(&mut self, msg: &rosc::OscMessage, cmds: &mut Vec<Cmd>) -> Result<(), String> {
        match msg.addr.as_str() {
            "/s_new" => {
                let [
                    OscType::String(name),
                    OscType::Int(id),
                    OscType::Int(action),
                    OscType::Int(target),
                    rest @ ..,
                ] = msg.args.as_slice()
                else {
                    return Err("expected: name, id, addAction, targetID".into());
                };
                let (mut synth, def) = self.make_synth(name)?;
                let action = AddAction::from_i32(*action).ok_or("add action must be 0-4")?;
                let id = if *id == -1 {
                    self.next_auto_id += 1;
                    self.next_auto_id
                } else if *id > 0 {
                    *id
                } else {
                    return Err("node ID must be positive or -1".into());
                };
                let mut controls = def.control_defaults();
                for pair in rest.chunks(2) {
                    if let (Some(index), Some(value)) = (
                        control_key(&pair[0], &def),
                        pair.get(1).and_then(float_value),
                    ) {
                        synth.set_control(index, value);
                        if let Some(slot) = controls.get_mut(index as usize) {
                            *slot = value;
                        }
                    }
                }
                let (usage, bus_controls) = def.usage(&controls);
                self.node_defs.insert(id, def);
                cmds.push(Cmd::AddSynth {
                    id,
                    target: *target,
                    action,
                    synth,
                    usage,
                });
                let body = MirrorBody::Synth {
                    def_name: name.clone(),
                    controls,
                    usage,
                    bus_controls,
                    maps: Vec::new(),
                };
                if let Ok(parent) = self.mirror.insert(id, body, *target, action) {
                    self.resort_from(Some(parent), cmds);
                }
                Ok(())
            }
            "/n_set" => {
                let Some(OscType::Int(id)) = msg.args.first() else {
                    return Err("expected: id, then control/value pairs".into());
                };
                // A GraphDef instance resolves names against its named surface
                // (never the private member nodes); a plain group propagates
                // to its subtree; a synth sets itself.
                if self.graph_set(*id, &msg.args[1..], cmds) {
                    return Ok(());
                }
                if self.node_unknown(*id) {
                    return Err(format!("node {id} not found"));
                }
                for node in self.control_targets(*id) {
                    let Some(def) = self.node_defs.get(&node).cloned() else {
                        continue;
                    };
                    let mut bus_control_hit = false;
                    for pair in msg.args[1..].chunks(2) {
                        if let (Some(index), Some(value)) = (
                            control_key(&pair[0], &def),
                            pair.get(1).and_then(float_value),
                        ) {
                            cmds.push(Cmd::SetControl {
                                id: node,
                                index,
                                value,
                            });
                            bus_control_hit |= self.mirror.set_control(node, index, value);
                            // An explicit set clears any mapping on that control
                            // (scsynth); dropping an audio map changes usage.
                            bus_control_hit |= self.mirror.set_map(node, index, -1, false);
                        }
                    }
                    if bus_control_hit {
                        self.reanalyze_and_resort(node, cmds);
                    }
                }
                Ok(())
            }
            "/n_map" => self.map_controls(msg, false, cmds),
            "/n_mapa" => self.map_controls(msg, true, cmds),
            "/n_setn" => self.set_controls_n(msg, cmds),
            "/n_fill" => self.fill_controls(msg, cmds),
            "/n_mapn" => self.map_controls_n(msg, false, cmds),
            "/n_mapan" => self.map_controls_n(msg, true, cmds),
            "/n_order" => self.order_nodes(msg, cmds),
            "/g_head" | "/g_tail" => self.move_to_group(msg, cmds),
            // M18: instantiate a GraphDef as a wired group with private buses.
            "/graph_new" => self.graph_new(msg, cmds),
            // M18: spawn a per-voice sub-graph inside an instance.
            "/graph_voice" => self.graph_voice(msg, cmds),
            // M17 MIDI binding config (no engine command; pure translator state).
            "/midi_bind" => self.midi_bind(msg, cmds),
            "/midi_unbind" => self.midi_unbind(msg, cmds),
            "/midi_map" => self.midi_map(msg),
            "/n_free" => {
                for arg in &msg.args {
                    let OscType::Int(id) = arg else {
                        return Err("expected int node IDs".into());
                    };
                    cmds.push(Cmd::FreeNode { id: *id });
                    // Removals keep any topological order valid: no re-sort.
                    self.mirror.remove(*id);
                    // Freeing a GraphDef voice or instance group drops its
                    // translator state / reclaims private buses (a no-op for
                    // ordinary nodes).
                    self.free_graph_node(*id);
                }
                Ok(())
            }
            "/n_run" => {
                // Pairs of (nodeID, flag): flag 0 pauses the node, non-zero
                // resumes it. A paused node stays in the tree (no mirror change).
                for pair in msg.args.chunks(2) {
                    let [OscType::Int(id), OscType::Int(flag)] = pair else {
                        return Err("expected int (nodeID, flag) pairs".into());
                    };
                    if self.node_unknown(*id) {
                        return Err(format!("node {id} not found"));
                    }
                    cmds.push(Cmd::RunNode {
                        id: *id,
                        run: *flag != 0,
                    });
                }
                Ok(())
            }
            "/n_before" | "/n_after" => {
                let place = if msg.addr == "/n_before" {
                    Place::Before
                } else {
                    Place::After
                };
                for pair in msg.args.chunks(2) {
                    let [OscType::Int(id), OscType::Int(target)] = pair else {
                        return Err("expected int (nodeID, targetID) pairs".into());
                    };
                    // Manual ordering is the auto group's job (M12).
                    for node in [*id, *target] {
                        if self
                            .mirror
                            .parent(node)
                            .is_some_and(|p| self.mirror.is_auto_group(p))
                        {
                            return Err(format!(
                                "node {node} is in an auto-sorted group (/g_sortMode): manual moves are disabled there"
                            ));
                        }
                    }
                    cmds.push(Cmd::MoveNode {
                        id: *id,
                        target: *target,
                        place,
                    });
                    // Reparenting can change the bus usage of auto ancestors.
                    if let Some((old_parent, new_parent)) =
                        self.mirror.move_node(*id, *target, place)
                    {
                        self.resort_from(Some(old_parent), cmds);
                        if new_parent != old_parent {
                            self.resort_from(Some(new_parent), cmds);
                        }
                    }
                }
                Ok(())
            }
            "/g_new" => {
                for triple in msg.args.chunks(3) {
                    let [OscType::Int(id), OscType::Int(action), OscType::Int(target)] = triple
                    else {
                        return Err("expected int (id, addAction, targetID) triples".into());
                    };
                    let action = AddAction::from_i32(*action).ok_or("add action must be 0-4")?;
                    if *id <= 0 {
                        return Err("group ID must be positive".into());
                    }
                    cmds.push(Cmd::AddGroup {
                        id: *id,
                        target: *target,
                        action,
                        group: self.new_group(),
                    });
                    let body = MirrorBody::Group {
                        children: Vec::new(),
                        auto: false,
                        parallel: false,
                    };
                    // An empty group has no bus usage: no re-sort needed.
                    let _ = self.mirror.insert(*id, body, *target, action);
                }
                Ok(())
            }
            "/g_freeAll" | "/g_deepFree" => {
                for arg in &msg.args {
                    let OscType::Int(id) = arg else {
                        return Err("expected int group IDs".into());
                    };
                    if msg.addr == "/g_freeAll" {
                        cmds.push(Cmd::FreeAllInGroup { id: *id });
                        self.mirror.free_all(*id);
                    } else {
                        cmds.push(Cmd::DeepFreeGroup { id: *id });
                        self.mirror.deep_free(*id);
                        self.free_graph_node(*id);
                    }
                }
                Ok(())
            }
            // M13: `/g_parallel groupID mode` — mode 1 runs the group's
            // children in dependency stages on the engine's worker pool
            // (sequential without workers); mode 0 returns to strict order.
            "/g_parallel" => {
                if msg.args.is_empty() || !msg.args.len().is_multiple_of(2) {
                    return Err("expected (groupID, mode) pairs".into());
                }
                for pair in msg.args.chunks(2) {
                    let [OscType::Int(group), OscType::Int(mode)] = pair else {
                        return Err("expected int (groupID, mode) pairs".into());
                    };
                    self.mirror.set_parallel(*group, *mode != 0)?;
                    cmds.push(Cmd::SetGroupParallel {
                        id: *group,
                        parallel: *mode != 0,
                    });
                }
                Ok(())
            }
            // M12: `/g_sortMode groupID mode` — mode 1 sorts the group's
            // children by their bus connections now and on every future
            // change; mode 0 returns it to manual ordering.
            "/g_sortMode" => {
                if msg.args.is_empty() || !msg.args.len().is_multiple_of(2) {
                    return Err("expected (groupID, mode) pairs".into());
                }
                for pair in msg.args.chunks(2) {
                    let [OscType::Int(group), OscType::Int(mode)] = pair else {
                        return Err("expected int (groupID, mode) pairs".into());
                    };
                    self.mirror.set_auto(*group, *mode != 0)?;
                    if *mode != 0 {
                        self.resort_from(Some(*group), cmds);
                    }
                }
                Ok(())
            }
            // The immediate form writes the shared atomics on the network
            // thread, but a scheduled write must land at its exact sample on
            // the engine.
            "/c_set" => {
                for pair in msg.args.chunks(2) {
                    let (OscType::Int(index), Some(value)) = (&pair[0], float_value(&pair[1]))
                    else {
                        return Err("expected (busIndex, value) pairs".into());
                    };
                    if *index < 0 {
                        return Err("bus index must be non-negative".into());
                    }
                    cmds.push(Cmd::SetControlBus {
                        index: *index as usize,
                        value,
                    });
                }
                Ok(())
            }
            "/c_setn" => self.set_control_bus_n(msg, cmds),
            "/c_fill" => self.fill_control_bus(msg, cmds),
            "/u_cmd" => self.ugen_command(msg, cmds),
            other => Err(format!("{other} cannot be scheduled in a timed bundle")),
        }
    }

    /// `/midi_bind channel instrument [target] [addAction] [gate]`: bind a MIDI
    /// channel to an instrument def (SynthDef *or* FaustDef *or* GraphDef).
    /// Default control map is `freq`/`amp`; `/midi_map` extends it. When the
    /// instrument is a **GraphDef** (with per-voice members), the shared
    /// instance is spawned now and each note becomes a `/graph_voice`.
    fn midi_bind(&mut self, msg: &rosc::OscMessage, cmds: &mut Vec<Cmd>) -> Result<(), String> {
        let [
            OscType::Int(channel),
            OscType::String(instrument),
            rest @ ..,
        ] = msg.args.as_slice()
        else {
            return Err("expected: channel, instrument [, target, addAction, gate]".into());
        };
        let channel = midi_channel(*channel)?;
        let target = int_arg(rest, 0).unwrap_or(0);
        let action = int_arg(rest, 1).unwrap_or(0);
        let gate = int_arg(rest, 2).unwrap_or(0) != 0;
        if AddAction::from_i32(action).is_none() {
            return Err("add action must be 0-4".into());
        }
        let mut binding = MidiBinding::new(instrument.clone(), target, action, gate);
        binding.graph_instance = self.bind_graph_instance(instrument, target, action, cmds)?;
        self.midi.channels.insert(channel, binding);
        Ok(())
    }

    /// If `instrument` names a GraphDef, spawn its shared instance now (so each
    /// note spawns a voice into it) and return the instance id; otherwise
    /// `None` (a plain def is `/s_new`'d per note). A GraphDef with no
    /// per-voice members is rejected — it has nothing to play per note. Shared
    /// by `/midi_bind` and the M19 binding restore.
    fn bind_graph_instance(
        &mut self,
        instrument: &str,
        target: i32,
        action: i32,
        cmds: &mut Vec<Cmd>,
    ) -> Result<Option<i32>, String> {
        if !self.graph_defs.contains_key(instrument) {
            return Ok(None);
        }
        if !self.graph_defs[instrument].has_voice_members() {
            return Err(format!(
                "GraphDef {instrument:?} has no per-voice members to bind to MIDI"
            ));
        }
        let instance = self.midi.alloc_id();
        let new = midi_message(
            "/graph_new",
            vec![
                OscType::String(instrument.to_string()),
                OscType::Int(instance),
                OscType::Int(action),
                OscType::Int(target),
            ],
        );
        self.graph_new(&new, cmds)?;
        Ok(Some(instance))
    }

    /// M19: re-establish a persisted binding at startup, re-instantiating its
    /// shared GraphDef instance if needed. Mirrors `/midi_bind` but takes the
    /// stored config directly (no re-issued OSC).
    pub fn restore_binding(
        &mut self,
        pb: crate::midi::PersistedBinding,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let mut binding = pb.binding;
        binding.graph_instance =
            self.bind_graph_instance(&binding.instrument, binding.target, binding.action, cmds)?;
        self.midi.channels.insert(pb.channel, binding);
        Ok(())
    }

    /// `/midi_unbind channel`: drop the binding and free every voice still
    /// sounding on that channel (and, for a GraphDef binding, its shared
    /// instance — which frees the voices with it).
    fn midi_unbind(&mut self, msg: &rosc::OscMessage, cmds: &mut Vec<Cmd>) -> Result<(), String> {
        let Some(OscType::Int(channel)) = msg.args.first() else {
            return Err("expected: channel".into());
        };
        let channel = midi_channel(*channel)?;
        let instance = self
            .midi
            .channels
            .remove(&channel)
            .and_then(|b| b.graph_instance);
        let voices = self.midi.drain_channel(channel);
        if let Some(instance) = instance {
            // Freeing the instance group frees every voice sub-graph with it.
            cmds.push(Cmd::FreeNode { id: instance });
            self.mirror.remove(instance);
            self.free_graph_node(instance);
        } else {
            for id in voices {
                cmds.push(Cmd::FreeNode { id });
                self.mirror.remove(id);
            }
        }
        Ok(())
    }

    /// `/midi_map channel selector name`: route a message type to a control.
    /// Selectors: `note`, `vel`, `gate`, `bend`,
    /// `pressure` (channel aftertouch), `poly` (per-note aftertouch), `ccN`
    /// (control change), `progN` (program → instrument def `name`).
    fn midi_map(&mut self, msg: &rosc::OscMessage) -> Result<(), String> {
        let [
            OscType::Int(channel),
            OscType::String(selector),
            OscType::String(name),
        ] = msg.args.as_slice()
        else {
            return Err("expected: channel, selector, name".into());
        };
        let channel = midi_channel(*channel)?;
        let binding = self
            .midi
            .channels
            .get_mut(&channel)
            .ok_or_else(|| format!("channel {channel} is not bound"))?;
        match selector.as_str() {
            "note" => binding.freq_control = name.clone(),
            "vel" | "velocity" => binding.amp_control = name.clone(),
            "gate" => binding.gate_control = name.clone(),
            "bend" => binding.bend_control = Some(name.clone()),
            "pressure" => binding.pressure_control = Some(name.clone()),
            "poly" => binding.poly_control = Some(name.clone()),
            s if s.starts_with("cc") => {
                let n: u8 = s[2..].parse().map_err(|_| "bad cc selector".to_string())?;
                binding.cc.insert(n, name.clone());
            }
            s if s.starts_with("prog") => {
                let n: u8 = s[4..]
                    .parse()
                    .map_err(|_| "bad prog selector".to_string())?;
                binding.programs.insert(n, name.clone());
            }
            other => return Err(format!("unknown MIDI selector {other:?}")),
        }
        Ok(())
    }

    /// M17: actuate nodes from a standard channel-voice MIDI message. Reuses
    /// the OSC path by synthesizing the equivalent `/s_new`/`/n_set`/`/n_free`,
    /// so a MIDI-driven voice is byte-identical to the OSC one. Unbound
    /// channels and unmapped expressive messages are silently ignored (a
    /// running MIDI stream must never error). Network thread only.
    pub fn translate_midi(
        &mut self,
        msg: ChannelVoiceMessage,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        use ChannelVoiceMessage::*;
        match msg {
            NoteOn {
                channel,
                note,
                velocity,
            } => {
                if velocity == 0 {
                    self.midi_note_off(channel, note, cmds)
                } else {
                    self.midi_note_on(channel, note, velocity, cmds)
                }
            }
            NoteOff { channel, note, .. } => self.midi_note_off(channel, note, cmds),
            PolyAftertouch {
                channel,
                note,
                pressure,
            } => {
                if let Some(ctrl) = self
                    .midi
                    .channels
                    .get(&channel)
                    .and_then(|b| b.poly_control.clone())
                    && let Some(&id) = self.midi.voices.get(&(channel, note))
                {
                    self.midi_set(id, &ctrl, convert::aftertouch2control(pressure), cmds);
                }
                Ok(())
            }
            ChannelAftertouch { channel, pressure } => {
                if let Some(ctrl) = self
                    .midi
                    .channels
                    .get(&channel)
                    .and_then(|b| b.pressure_control.clone())
                {
                    self.midi_set_channel(
                        channel,
                        &ctrl,
                        convert::aftertouch2control(pressure),
                        cmds,
                    );
                }
                Ok(())
            }
            ControlChange {
                channel,
                controller,
                value,
            } => {
                if let Some(ctrl) = self
                    .midi
                    .channels
                    .get(&channel)
                    .and_then(|b| b.cc.get(&controller).cloned())
                {
                    self.midi_set_channel(channel, &ctrl, convert::cc2control(value), cmds);
                }
                Ok(())
            }
            PitchBend { channel, value } => {
                if let Some(ctrl) = self
                    .midi
                    .channels
                    .get(&channel)
                    .and_then(|b| b.bend_control.clone())
                {
                    self.midi_set_channel(channel, &ctrl, convert::bend2control(value), cmds);
                }
                Ok(())
            }
            ProgramChange { channel, program } => {
                if let Some(binding) = self.midi.channels.get_mut(&channel)
                    && let Some(instrument) = binding.programs.get(&program).cloned()
                {
                    binding.instrument = instrument;
                }
                Ok(())
            }
        }
    }

    /// Note on → `/s_new` with `freq`/`amp` from the conversions. Retriggering
    /// a note already sounding frees the old voice first.
    fn midi_note_on(
        &mut self,
        channel: u8,
        note: u8,
        velocity: u16,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let Some(binding) = self.midi.channels.get(&channel) else {
            return Ok(());
        };
        let instrument = binding.instrument.clone();
        let target = binding.target;
        let action = binding.action;
        let freq_control = binding.freq_control.clone();
        let amp_control = binding.amp_control.clone();
        let graph_instance = binding.graph_instance;
        if self.midi.voices.contains_key(&(channel, note)) {
            self.midi_note_off(channel, note, cmds)?;
        }
        let id = self.midi.alloc_id();
        let freq = OscType::Float(convert::midi2freq(note as f32));
        let amp = OscType::Float(convert::velocity2amp(velocity));
        // A GraphDef binding spawns a per-voice sub-graph into the shared
        // instance; a plain def spawns a synth. Both carry freq/amp as the
        // surface/control values.
        let msg = match graph_instance {
            Some(instance) => midi_message(
                "/graph_voice",
                vec![
                    OscType::Int(instance),
                    OscType::Int(id),
                    OscType::String(freq_control),
                    freq,
                    OscType::String(amp_control),
                    amp,
                ],
            ),
            None => midi_message(
                "/s_new",
                vec![
                    OscType::String(instrument),
                    OscType::Int(id),
                    OscType::Int(action),
                    OscType::Int(target),
                    OscType::String(freq_control),
                    freq,
                    OscType::String(amp_control),
                    amp,
                ],
            ),
        };
        self.translate(&msg, cmds)?;
        self.midi.voices.insert((channel, note), id);
        Ok(())
    }

    /// Note off → `/n_free` (or `/n_set gate 0` for gate-aware bindings).
    fn midi_note_off(&mut self, channel: u8, note: u8, cmds: &mut Vec<Cmd>) -> Result<(), String> {
        let Some(id) = self.midi.voices.remove(&(channel, note)) else {
            return Ok(());
        };
        let gate = self.midi.channels.get(&channel);
        let msg = match gate.filter(|b| b.gate) {
            Some(b) => midi_message(
                "/n_set",
                vec![
                    OscType::Int(id),
                    OscType::String(b.gate_control.clone()),
                    OscType::Float(0.0),
                ],
            ),
            None => midi_message("/n_free", vec![OscType::Int(id)]),
        };
        // A freed voice may already be gone; an unknown control is a no-op.
        let _ = self.translate(&msg, cmds);
        Ok(())
    }

    /// `/n_set` one control on one voice; tolerate a stale node / unknown name.
    fn midi_set(&mut self, id: i32, control: &str, value: f32, cmds: &mut Vec<Cmd>) {
        let msg = midi_message(
            "/n_set",
            vec![
                OscType::Int(id),
                OscType::String(control.to_string()),
                OscType::Float(value),
            ],
        );
        let _ = self.translate(&msg, cmds);
    }

    /// `/n_set` one control on every live voice of a channel.
    fn midi_set_channel(&mut self, channel: u8, control: &str, value: f32, cmds: &mut Vec<Cmd>) {
        for id in self.midi.voice_ids(channel) {
            self.midi_set(id, control, value, cmds);
        }
    }

    /// `/g_queryTree.reply` arguments, scsynth-compatible: `flag`, the
    /// queried group and its child count, then depth-first per node: ID and
    /// child count (`-1` for synths), the def name for synths, and — with
    /// `flag` — the control count and (name, value) pairs.
    pub fn query_tree(&self, group: i32, with_controls: bool) -> Result<Vec<OscType>, String> {
        let Some(children) = self.mirror.children(group) else {
            return Err(match self.mirror.get(group) {
                Some(_) => format!("node {group} is not a group"),
                None => format!("group {group} not found"),
            });
        };
        let mut args = vec![
            OscType::Int(with_controls as i32),
            OscType::Int(group),
            OscType::Int(children.len() as i32),
        ];
        self.query_children(group, with_controls, &mut args);
        Ok(args)
    }

    fn query_children(&self, group: i32, with_controls: bool, args: &mut Vec<OscType>) {
        let children = self.mirror.children(group).unwrap_or(&[]).to_vec();
        for child in children {
            args.push(OscType::Int(child));
            if let Some(grandchildren) = self.mirror.children(child) {
                args.push(OscType::Int(grandchildren.len() as i32));
                self.query_children(child, with_controls, args);
            } else if let Some((def_name, controls)) = self.mirror.synth_info(child) {
                args.push(OscType::Int(-1));
                args.push(OscType::String(def_name.into()));
                if with_controls {
                    args.push(OscType::Int(controls.len() as i32));
                    let def = self.node_defs.get(&child);
                    for (i, value) in controls.iter().enumerate() {
                        let name = def.and_then(|d| d.control_name(i)).unwrap_or("");
                        if name.is_empty() {
                            args.push(OscType::Int(i as i32));
                        } else {
                            args.push(OscType::String(name.into()));
                        }
                        args.push(OscType::Float(*value));
                    }
                }
            }
        }
    }

    /// `/n_info` arguments for `/n_query`: per-node detail beyond the tree
    /// structure `/g_queryTree` gives. Layout: `nodeID, parentID, prevID,
    /// nextID, isGroup`; then for a **group** `headID, tailID` (`-1` if empty);
    /// for a **synth** `defName`, `numControls` + (name|index, value) pairs,
    /// `numMaps` + (controlIndex, bus, audio) triples, and the inferred
    /// `reads`/`writes` bus lists as two strings (same format as
    /// `/g_dumpGraph`). Siblings are `-1` when absent.
    pub fn node_info(&self, id: i32) -> Result<Vec<OscType>, String> {
        let Some(node) = self.mirror.get(id) else {
            return Err(format!("node {id} not found"));
        };
        let parent = self.mirror.parent(id).unwrap_or(-1);
        let (prev, next) = self.siblings(id, parent);
        let mut args = vec![
            OscType::Int(id),
            OscType::Int(parent),
            OscType::Int(prev),
            OscType::Int(next),
        ];
        match &node.body {
            MirrorBody::Group { children, .. } => {
                args.push(OscType::Int(1));
                args.push(OscType::Int(children.first().copied().unwrap_or(-1)));
                args.push(OscType::Int(children.last().copied().unwrap_or(-1)));
            }
            MirrorBody::Synth {
                def_name,
                controls,
                maps,
                ..
            } => {
                args.push(OscType::Int(0));
                args.push(OscType::String(def_name.clone()));
                args.push(OscType::Int(controls.len() as i32));
                let def = self.node_defs.get(&id);
                for (i, value) in controls.iter().enumerate() {
                    let name = def.and_then(|d| d.control_name(i)).unwrap_or("");
                    if name.is_empty() {
                        args.push(OscType::Int(i as i32));
                    } else {
                        args.push(OscType::String(name.into()));
                    }
                    args.push(OscType::Float(*value));
                }
                args.push(OscType::Int(maps.len() as i32));
                for (ctl, bus, audio) in maps {
                    args.push(OscType::Int(*ctl as i32));
                    args.push(OscType::Int(*bus));
                    args.push(OscType::Int(*audio as i32));
                }
                let usage = self.mirror.usage_of(id);
                args.push(OscType::String(bus_list(usage.reads)));
                args.push(OscType::String(bus_list(usage.writes)));
            }
        }
        Ok(args)
    }

    /// Previous and next sibling of `id` within `parent`'s children (`-1` if
    /// there is none or `id` is the root).
    fn siblings(&self, id: i32, parent: i32) -> (i32, i32) {
        let Some(sibs) = self.mirror.children(parent) else {
            return (-1, -1);
        };
        let Some(pos) = sibs.iter().position(|&c| c == id) else {
            return (-1, -1);
        };
        let prev = if pos > 0 { sibs[pos - 1] } else { -1 };
        let next = sibs.get(pos + 1).copied().unwrap_or(-1);
        (prev, next)
    }

    /// `/g_dumpGraph`: a human-readable view of the inferred bus graph of
    /// one group — what each child reads/writes and the current order.
    pub fn dump_graph(&self, group: i32) -> Result<String, String> {
        let Some(children) = self.mirror.children(group) else {
            return Err(match self.mirror.get(group) {
                Some(_) => format!("node {group} is not a group"),
                None => format!("group {group} not found"),
            });
        };
        let auto = if self.mirror.is_auto_group(group) {
            "auto"
        } else {
            "manual"
        };
        let parallel = if self.mirror.is_parallel_group(group) {
            ", parallel"
        } else {
            ""
        };
        let mut out = format!("group {group} ({auto}{parallel})\n");
        for &child in children {
            let usage = self.mirror.usage_of(child);
            let kind = match self.mirror.synth_info(child) {
                Some((def_name, _)) => def_name.to_string(),
                None if self.mirror.is_auto_group(child) => "group (auto)".into(),
                None => "group".into(),
            };
            let dynamic = if usage.dynamic { "  dynamic" } else { "" };
            out.push_str(&format!(
                "  {child} {kind}  reads {}  writes {}{dynamic}\n",
                bus_list(usage.reads),
                bus_list(usage.writes),
            ));
        }
        Ok(out)
    }
}

/// `u128` bus mask → "0,1,16" (or "-" when empty).
fn bus_list(mask: u128) -> String {
    if mask == 0 {
        return "-".into();
    }
    let buses: Vec<String> = (0..128)
        .filter(|b| mask & (1 << b) != 0)
        .map(|b| b.to_string())
        .collect();
    buses.join(",")
}

/// Parses one `/b_*` command (except the synchronous `/b_query`) into the
/// buffer index and the NRT job that performs it. `mirror` is the
/// network-side pool: commands that keep or reuse the current contents
/// (`/b_read`, `/b_write`, `/b_zero`) read shape and data from it.
pub fn parse_buffer_msg(
    addr: &str,
    args: &[OscType],
    mirror: &BufferPool,
    default_sample_rate: f64,
) -> Result<(i32, NrtJob), String> {
    let (index, job) = match addr {
        "/b_alloc" => {
            let (index, frames) = match args {
                [OscType::Int(index), OscType::Int(frames), ..] => (*index, *frames),
                _ => return Err("expected: bufnum, frames [, channels]".into()),
            };
            let channels = int_arg(args, 2).unwrap_or(1);
            if frames <= 0 || channels <= 0 {
                return Err("frames and channels must be positive".into());
            }
            (
                index,
                NrtJob::Alloc {
                    frames: frames as usize,
                    channels: channels as usize,
                    sample_rate: default_sample_rate,
                },
            )
        }
        "/b_allocRead" => {
            let (index, path) = match args {
                [OscType::Int(index), OscType::String(path), ..] => (*index, path.clone()),
                _ => return Err("expected: bufnum, path [, fileStart, numFrames]".into()),
            };
            (
                index,
                NrtJob::AllocRead {
                    path,
                    file_start: int_arg(args, 2).unwrap_or(0).max(0) as usize,
                    num_frames: int_arg(args, 3).unwrap_or(0) as i64,
                },
            )
        }
        // `leaveOpen` is accepted and ignored (no streaming yet). The buffer
        // must already exist; its shape is kept.
        "/b_read" => {
            let (index, path) = match args {
                [OscType::Int(index), OscType::String(path), ..] => (*index, path.clone()),
                _ => return Err("expected: bufnum, path [, fileStart, numFrames, bufStart]".into()),
            };
            let Some(current) = mirror_buffer(mirror, index) else {
                return Err(format!("no buffer allocated at {index}"));
            };
            (
                index,
                NrtJob::Read {
                    path,
                    file_start: int_arg(args, 2).unwrap_or(0).max(0) as usize,
                    num_frames: int_arg(args, 3).unwrap_or(-1) as i64,
                    buf_start: int_arg(args, 4).unwrap_or(0).max(0) as usize,
                    current,
                },
            )
        }
        // WAV only in v1.
        "/b_write" => {
            let (index, path) = match args {
                [OscType::Int(index), OscType::String(path), ..] => (*index, path.clone()),
                _ => {
                    return Err(
                        "expected: bufnum, path [, headerFormat, sampleFormat, numFrames, startFrame]"
                            .into(),
                    );
                }
            };
            let header = string_arg(args, 2).unwrap_or("wav");
            if !header.eq_ignore_ascii_case("wav") && !header.eq_ignore_ascii_case("wave") {
                return Err(format!("unsupported header format {header:?}"));
            }
            let Some(buffer) = mirror_buffer(mirror, index) else {
                return Err(format!("no buffer allocated at {index}"));
            };
            (
                index,
                NrtJob::Write {
                    path,
                    sample_format: string_arg(args, 3).unwrap_or("int16").to_string(),
                    num_frames: int_arg(args, 4).unwrap_or(-1) as i64,
                    buf_start: int_arg(args, 5).unwrap_or(0).max(0) as usize,
                    buffer,
                },
            )
        }
        // Buffers are immutable: zeroing builds a same-shape replacement.
        "/b_zero" => {
            let Some(OscType::Int(index)) = args.first() else {
                return Err("expected a buffer index".into());
            };
            let Some(current) = mirror_buffer(mirror, *index) else {
                return Err(format!("no buffer allocated at {index}"));
            };
            (
                *index,
                NrtJob::Alloc {
                    frames: current.frames(),
                    channels: current.channels(),
                    sample_rate: current.sample_rate(),
                },
            )
        }
        "/b_free" => {
            let Some(OscType::Int(index)) = args.first() else {
                return Err("expected a buffer index".into());
            };
            (*index, NrtJob::Free)
        }
        other => return Err(format!("{other} is not a buffer command")),
    };
    // The mirror pool is sized to the boot-time `--max-buffers`, so its length
    // is the authoritative index bound.
    if index < 0 || index as usize >= mirror.len() {
        return Err(format!("buffer index out of range: {index}"));
    }
    Ok((index, job))
}

/// Parses a `/b_gen bufnum cmd ...` command into the buffer index and the NRT
/// job that fills it. The named `cmd` selects a generator (`sine1`/`sine2`/
/// `sine3`/`cheby`) or `copy`; the flag int and the trailing floats are pulled
/// per command. Needs an allocated buffer (its shape drives generation), read
/// from `mirror` — so a `/b_gen` right after a `/b_alloc` needs a `/sync`
/// between them, exactly like `/b_read`.
pub fn parse_b_gen(args: &[OscType], mirror: &BufferPool) -> Result<(i32, NrtJob), String> {
    use crate::dsp::wavetable::{GenCommand, GenFlags};

    let (index, cmd) = match args {
        [OscType::Int(index), OscType::String(cmd), ..] => (*index, cmd.as_str()),
        _ => return Err("expected: bufnum, command name, args...".into()),
    };
    if index < 0 || index as usize >= mirror.len() {
        return Err(format!("buffer index out of range: {index}"));
    }
    let Some(current) = mirror_buffer(mirror, index) else {
        return Err(format!("no buffer allocated at {index}"));
    };
    let rest = &args[2..];

    let command = match cmd {
        "copy" => {
            // copy dstStart srcBufnum srcStart numSamples
            let [
                OscType::Int(dst_start),
                OscType::Int(src_buf),
                OscType::Int(src_start),
                OscType::Int(num),
            ] = rest
            else {
                return Err("copy expects: dstStart, srcBufnum, srcStart, numSamples".into());
            };
            let Some(src) = mirror_buffer(mirror, *src_buf) else {
                return Err(format!("no source buffer allocated at {src_buf}"));
            };
            GenCommand::Copy {
                dst_start: (*dst_start).max(0) as usize,
                src,
                src_start: (*src_start).max(0) as usize,
                num: *num as i64,
            }
        }
        "sine1" | "sine2" | "sine3" | "cheby" => {
            let Some((OscType::Int(flag_bits), tail)) = rest.split_first() else {
                return Err(format!("{cmd} expects: flags, then values"));
            };
            let flags = GenFlags::from_bits(*flag_bits);
            let values: Vec<f32> = tail.iter().filter_map(float_value).collect();
            match cmd {
                "sine1" => GenCommand::Sine1 {
                    flags,
                    amps: values,
                },
                "cheby" => GenCommand::Cheby {
                    flags,
                    coeffs: values,
                },
                "sine2" => GenCommand::Sine2 {
                    flags,
                    partials: values.chunks_exact(2).map(|c| (c[0], c[1])).collect(),
                },
                // sine3
                _ => GenCommand::Sine3 {
                    flags,
                    partials: values.chunks_exact(3).map(|c| (c[0], c[1], c[2])).collect(),
                },
            }
        }
        other => return Err(format!("unknown /b_gen command {other:?}")),
    };
    Ok((
        index,
        NrtJob::Gen {
            current,
            cmd: command,
        },
    ))
}

/// `/d_faust name payload` arguments: the payload string is Faust source or
/// a JSON box tree (the caller sniffs the leading `{`).
pub fn parse_d_faust(args: &[OscType]) -> Result<(String, String), String> {
    let (name, def) = match args {
        [OscType::String(name), OscType::String(src), ..] => (name.clone(), src.clone()),
        [OscType::String(name), OscType::Blob(src), ..] => (
            name.clone(),
            String::from_utf8(src.clone()).map_err(|_| "def blob is not UTF-8".to_string())?,
        ),
        _ => return Err("expected: name, JSON or Faust source".into()),
    };
    if name.is_empty() {
        return Err("empty def name".into());
    }
    Ok((name, def))
}

fn mirror_buffer(mirror: &BufferPool, index: i32) -> Option<Arc<Buffer>> {
    usize::try_from(index)
        .ok()
        .and_then(|i| mirror.get(i))
        .and_then(|b| b.as_ref().map(Arc::clone))
}

/// A MIDI channel argument: 0-based, the classic 16 plus the extended UMP
/// group×channel space (0..=255).
fn midi_channel(channel: i32) -> Result<u8, String> {
    u8::try_from(channel).map_err(|_| "MIDI channel out of range (0-255)".to_string())
}

/// Builds the OSC message a MIDI event is realized as, fed back through
/// [`CmdTranslator::translate`] for byte-identical parity with the OSC path.
fn midi_message(addr: &str, args: Vec<OscType>) -> rosc::OscMessage {
    rosc::OscMessage {
        addr: addr.to_string(),
        args,
    }
}

/// Control reference: by name (resolved against the def) or by index.
pub fn control_key(arg: &OscType, def: &NodeDef) -> Option<u32> {
    match arg {
        OscType::String(name) => def.control_index(name),
        OscType::Int(i) if *i >= 0 => Some(*i as u32),
        _ => None,
    }
}

/// Optional trailing int argument (scsynth buffer commands have several).
pub fn int_arg(args: &[OscType], n: usize) -> Option<i32> {
    match args.get(n) {
        Some(OscType::Int(i)) => Some(*i),
        _ => None,
    }
}

pub fn string_arg(args: &[OscType], n: usize) -> Option<&str> {
    match args.get(n) {
        Some(OscType::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

pub fn float_value(arg: &OscType) -> Option<f32> {
    match arg {
        OscType::Float(f) => Some(*f),
        OscType::Int(i) => Some(*i as f32),
        OscType::Double(d) => Some(*d as f32),
        _ => None,
    }
}

/// A bus index argument (`/n_map`/`/n_mapa`): a plain int, `-1` to unbind.
pub fn int_value(arg: &OscType) -> Option<i32> {
    match arg {
        OscType::Int(i) => Some(*i),
        _ => None,
    }
}
