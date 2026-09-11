//! Instancing a GraphDef: `/graph_new`, `/graph_addSlot` and `/graph_newVoice`.
//!
//! A GraphDef is a wiring, not a sound: the spec names member defs, the
//! private buses between them and the surface ports a client sets. Building
//! one means allocating those buses and the members' node ids, instantiating
//! each member with its wiring baked into the reserved `out`/`in` controls,
//! and resolving the surface into the (node, control) pairs a port drives.
//!
//! # Two passes, because a member may be another graph
//!
//! A member is one node or a whole nested graph, and a graph's members may be
//! graphs in turn — which is what lets a track hold clips and a clip hold an
//! effect chain without either of those being a second mechanism. That makes
//! instantiation a **tree**, and a tree cannot be built the way one level was:
//! a child that fails halfway would leave its siblings standing.
//!
//! So it is planned and then realized. [`Planned`] is the whole tree with every
//! fallible thing already done — every def resolved, every synth built, every
//! bus and node id taken — and holds enough to hand all of it back if any part
//! of the walk fails ([`Planned::release`]). Realizing it emits commands and
//! cannot fail. The all-or-nothing rule the one-level version had is therefore
//! the same rule, over a tree: **a rejected instantiation leaves the pools
//! exactly as it found them.**
//!
//! The nesting is of authoring and not of execution. A child is a subgroup, its
//! surface is spliced flat into its parent's at resolve time, and the audio
//! thread sees groups and synths as it always did.

use super::*;

/// Result of a GraphDef bus allocation: the symbolic-name → first-index map,
/// plus the `(first, width)` runs taken from the audio and control pools (in
/// that order), kept so teardown can hand them back.
type GraphBusAlloc = (
    HashMap<String, usize>,
    Vec<(usize, usize)>,
    Vec<(usize, usize)>,
);

impl CmdTranslator {
    /// The ids of a GraphDef instantiation, all or nothing: the group (auto
    /// when `id_arg` is -1, the client's otherwise) plus one per member. A
    /// shortfall hands back every id it took.
    fn alloc_graph_ids(&mut self, id_arg: i32, members: usize) -> Result<(i32, Vec<i32>), String> {
        let group_id = if id_arg == -1 {
            self.alloc_auto_id()?
        } else {
            id_arg
        };
        match self.alloc_auto_ids(members) {
            Ok(member_ids) => Ok((group_id, member_ids)),
            Err(e) => {
                if id_arg == -1 {
                    self.release_auto_ids(&[group_id]);
                }
                Err(e)
            }
        }
    }

    /// Hands a failed instantiation's private buses back to their pools.
    fn free_graph_buses(&mut self, audio: &[(usize, usize)], control: &[(usize, usize)]) {
        for &(f, w) in audio {
            let _ = self.graph_audio_buses.release(f as i64, w);
        }
        for &(f, w) in control {
            let _ = self.graph_control_buses.release(f as i64, w);
        }
    }

    /// Allocates a GraphDef's private buses (resolved name → first index).
    /// On a shortfall it hands back everything it took, so the caller's later
    /// steps stay side-effect-free until this succeeds. Returns the name→index
    /// map plus the `(first, width)` audio and control allocations.
    fn alloc_graph_buses(
        &mut self,
        def: &GraphDefSpec,
        external: &HashMap<String, usize>,
    ) -> Result<GraphBusAlloc, String> {
        let mut bus_index = HashMap::new();
        let mut audio: Vec<(usize, usize)> = Vec::new();
        let mut control: Vec<(usize, usize)> = Vec::new();
        for b in &def.buses {
            // **A bus the caller already has is not allocated here**, and is
            // not reclaimed here either. That is one rule serving two things
            // that turn out to be the same: a nested graph handed its parent's
            // output, and a slot built against the instance's own buses (which
            // is what "wired to the same private buses" has always meant). A
            // graph instantiated on its own is handed nothing and allocates
            // everything, so one def works standalone and nested.
            if let Some(&given) = external.get(b.name.as_str()) {
                bus_index.insert(b.name.clone(), given);
                continue;
            }
            let width = b.channels.max(1);
            let first = match b.rate {
                BusRate::Audio => self.graph_audio_buses.alloc(width),
                BusRate::Control => self.graph_control_buses.alloc(width),
            };
            let Some(first) = first else {
                for (f, w) in audio {
                    let _ = self.graph_audio_buses.release(f as i64, w);
                }
                for (f, w) in control {
                    let _ = self.graph_control_buses.release(f as i64, w);
                }
                return Err("out of private buses for GraphDef".into());
            };
            let first = first as usize;
            match b.rate {
                BusRate::Audio => audio.push((first, width)),
                BusRate::Control => control.push((first, width)),
            }
            bus_index.insert(b.name.clone(), first);
        }
        Ok((bus_index, audio, control))
    }

    /// Instantiates the members at `indices` inside `parent`, consuming the
    /// pre-built synths and pre-allocated node ids (both parallel to
    /// `indices`): sets each control (bus references resolved against
    /// `bus_index`, `"OUT"` → bus 0) and applies the `/node_map` wiring. Returns
    /// member index → node id. Infallible — the fallible `make_synth` and id
    /// allocation happened in the caller, so an instance is never left
    /// half-built.
    #[allow(clippy::too_many_arguments)]
    fn build_members(
        &mut self,
        def: &GraphDefSpec,
        indices: &[usize],
        built: Vec<(Box<dyn SynthNode>, NodeDef)>,
        ids: Vec<i32>,
        parent: i32,
        bus_index: &HashMap<String, usize>,
        cmds: &mut Vec<Cmd>,
    ) -> HashMap<usize, i32> {
        let mut node_of: HashMap<usize, i32> = HashMap::new();
        for ((&mi, (mut synth, ndef)), node_id) in indices.iter().zip(built).zip(ids) {
            let member = &def.members[mi];
            let mut controls = ndef.control_defaults();
            for (cname, cval) in &member.controls {
                let Some(index) = ndef.control_index(cname) else {
                    continue;
                };
                let value = match cval {
                    ControlValue::Num(v) => *v,
                    // validate() guaranteed the name resolves and the channel
                    // is inside the bus. `OUT` is the hardware, whose first
                    // channel is bus 0.
                    ControlValue::Bus(b) => {
                        let (name, channel) = bus_channel(b);
                        match name {
                            "OUT" => channel as f32,
                            name => (bus_index[name] + channel) as f32,
                        }
                    }
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
        // `/node_map` wiring, once every member exists.
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

    /// Resolves the surface ports whose targets are *all* present in `node_of`
    /// or `child_of` → `(node id, control index, mul, add)`. So passing the
    /// shared maps yields the shared ports and passing a slot's maps yields
    /// that slot's ports (a port never mixes the two — see `validate`).
    ///
    /// A target on a **nested graph** resolves through that child's own
    /// resolved surface, and the two scalings compose: the outer runs first, so
    /// the pair that lands is `(mul_in·mul_out, mul_in·add_out + add_in)`. What
    /// comes out is flat — the port of a track that drives a control of a node
    /// three levels down is one `/node_set` like every other.
    fn resolve_ports(
        &self,
        def: &GraphDefSpec,
        node_of: &HashMap<usize, i32>,
        child_of: &HashMap<usize, i32>,
    ) -> ResolvedSurface {
        let mut surface = ResolvedSurface::new();
        for (port, targets) in &def.surface {
            if !targets
                .iter()
                .all(|t| node_of.contains_key(&t.member) || child_of.contains_key(&t.member))
            {
                continue;
            }
            let mut resolved = Vec::new();
            for t in targets {
                if let Some(&child) = child_of.get(&t.member) {
                    let inner = t.port.as_deref().and_then(|p| {
                        self.graph_instances
                            .get(&child)
                            .and_then(|i| i.surface.get(p))
                    });
                    if let Some(inner) = inner {
                        for &(node, index, mul, add) in inner {
                            resolved.push((node, index, mul * t.mul, mul * t.add + add));
                        }
                    }
                    continue;
                }
                let node = node_of[&t.member];
                if let Some(index) = self
                    .node_defs
                    .get(&node)
                    .and_then(|d| d.control_index(&t.control))
                {
                    resolved.push((node, index, t.mul, t.add));
                }
            }
            surface.insert(port.clone(), resolved);
        }
        surface
    }

    /// Collects the per-instantiation `port value` overrides trailing a
    /// `/graph_new`/`/graph_newVoice`.
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
}

/// One instantiation, with every fallible step already done.
///
/// This is what makes a **tree** of graphs all-or-nothing: the whole walk
/// happens here, taking synths, buses and node ids as it goes, and if any part
/// of it fails what has been taken is handed back ([`CmdTranslator::release_plan`])
/// before anything observable has happened. Realizing a plan emits commands and
/// cannot fail.
struct Planned {
    def: Arc<GraphDefSpec>,
    /// The group this instantiation will be.
    group_id: i32,
    /// Whether `group_id` came from the auto pool (and so goes back to it on a
    /// failure) rather than from the client.
    owned_group: bool,
    /// The member indices built as nodes here, and their pre-built synths and
    /// ids, parallel to each other.
    def_members: Vec<usize>,
    built: Vec<(Box<dyn SynthNode>, NodeDef)>,
    member_ids: Vec<i32>,
    bus_index: HashMap<String, usize>,
    /// Only the buses this instantiation **allocated**: an external one is the
    /// parent's and is neither taken nor released here.
    audio_buses: Vec<(usize, usize)>,
    control_buses: Vec<(usize, usize)>,
    /// Member index → the plan of the nested graph it is.
    children: Vec<(usize, Planned)>,
}

impl CmdTranslator {
    /// Hands back everything a plan took: its children first, then its ids and
    /// its buses. Called on the failure of any step of the walk.
    fn release_plan(&mut self, plan: Planned) {
        for (_, child) in plan.children {
            self.release_plan(child);
        }
        self.release_auto_ids(&plan.member_ids);
        if plan.owned_group {
            self.release_auto_ids(&[plan.group_id]);
        }
        self.free_graph_buses(&plan.audio_buses, &plan.control_buses);
    }

    /// Walks a GraphDef and everything nested in it, taking what building it
    /// will need. `members` picks which members belong to this instantiation —
    /// the shared ones for `/graph_new`, one slot's for `/graph_addSlot` —
    /// and `external` names the buses the caller provides.
    fn plan_instance(
        &mut self,
        name: &str,
        id_arg: i32,
        slot: Option<&str>,
        external: &HashMap<String, usize>,
        depth: usize,
    ) -> Result<Planned, String> {
        if depth > MAX_GRAPH_DEPTH {
            return Err(format!(
                "GraphDef '{name}': nested more than {MAX_GRAPH_DEPTH} deep (a cycle?)"
            ));
        }
        let def = self
            .graph_defs
            .get(name)
            .cloned()
            .ok_or_else(|| format!("GraphDef not found: {name}"))?;
        let mine: Vec<usize> = def
            .members
            .iter()
            .enumerate()
            .filter(|(_, m)| m.slot() == slot)
            .map(|(i, _)| i)
            .collect();
        let def_members: Vec<usize> = mine
            .iter()
            .copied()
            .filter(|&i| def.members[i].kind == MemberKind::Def)
            .collect();
        let graph_members: Vec<usize> = mine
            .iter()
            .copied()
            .filter(|&i| def.members[i].kind == MemberKind::Graph)
            .collect();

        // --- fallible, in the order that leaves least to hand back.
        let mut built = Vec::with_capacity(def_members.len());
        for &mi in &def_members {
            built.push(self.make_synth(&def.members[mi].def)?);
        }
        let (bus_index, audio_buses, control_buses) = self.alloc_graph_buses(&def, external)?;
        let ids = match self.alloc_graph_ids(id_arg, def_members.len()) {
            Ok(ids) => ids,
            Err(e) => {
                self.free_graph_buses(&audio_buses, &control_buses);
                return Err(e);
            }
        };
        let (group_id, member_ids) = ids;
        let mut plan = Planned {
            def: Arc::clone(&def),
            group_id,
            owned_group: id_arg == -1,
            def_members,
            built,
            member_ids,
            bus_index,
            audio_buses,
            control_buses,
            children: Vec::new(),
        };

        // The nested graphs, each handed the buses this one names for it. A
        // failure past here hands back the whole plan, children included.
        for mi in graph_members {
            let member = &def.members[mi];
            let mut given = HashMap::new();
            for (child_bus, value) in &member.controls {
                let index = match value {
                    ControlValue::Bus(b) => {
                        let (bus, channel) = bus_channel(b);
                        if bus == "OUT" {
                            given.insert(child_bus.clone(), channel);
                            continue;
                        }
                        match plan.bus_index.get(bus) {
                            Some(&i) => i + channel,
                            None => {
                                let name = member.def.clone();
                                self.release_plan(plan);
                                return Err(format!(
                                    "member {mi} ('{name}'): unknown internal bus '{b}'"
                                ));
                            }
                        }
                    }
                    ControlValue::Num(v) => *v as usize,
                };
                given.insert(child_bus.clone(), index);
            }
            match self.plan_instance(&member.def.clone(), -1, None, &given, depth + 1) {
                Ok(child) => plan.children.push((mi, child)),
                Err(e) => {
                    self.release_plan(plan);
                    return Err(e);
                }
            }
        }
        Ok(plan)
    }

    /// Builds a planned instantiation: the group, its members, its nested
    /// graphs, and the resolved surface. Infallible by construction — every
    /// fallible step happened in [`CmdTranslator::plan_instance`] — so an
    /// instance is never left half-built.
    ///
    /// Registers the instance (or, with `slot`, the slot sub-graph) and answers
    /// its group id.
    fn realize(
        &mut self,
        plan: Planned,
        parent: i32,
        action: AddAction,
        slot: Option<(i32, &str)>,
        cmds: &mut Vec<Cmd>,
    ) -> i32 {
        let Planned {
            def,
            group_id,
            def_members,
            built,
            member_ids,
            bus_index,
            audio_buses,
            control_buses,
            children,
            ..
        } = plan;
        cmds.push(Cmd::AddGroup {
            id: group_id,
            target: parent,
            action,
            group: self.new_group(),
        });
        let _ = self
            .mirror
            .insert(group_id, MirrorBody::group(true), parent, action);
        let node_of = self.build_members(
            &def,
            &def_members,
            built,
            member_ids,
            group_id,
            &bus_index,
            cmds,
        );
        // The nested graphs, inside this group. Each registers itself, which is
        // what makes its surface reachable when this one's ports resolve.
        let mut child_of = HashMap::new();
        for (mi, child) in children {
            let id = self.realize(child, group_id, AddAction::Tail, None, cmds);
            child_of.insert(mi, id);
        }
        self.resort_from(Some(group_id), cmds);
        let mut surface = self.resolve_ports(&def, &node_of, &child_of);
        // **A slot that *is* one graph answers that graph's ports too.** A slot
        // of clips holds one `mt.clip` and nothing else, so the group a caller
        // was handed would otherwise answer only the ports the *parent's* def
        // thought to re-export, and the clip's own surface would be reachable
        // by no id the caller has. Its ports are spliced in rather than
        // replacing anything: an explicit re-export still wins.
        if slot.is_some()
            && node_of.is_empty()
            && let [(_, only)] = child_of.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>()[..]
            && let Some(child) = self.graph_instances.get(&only)
        {
            for (port, targets) in child.surface.clone() {
                surface.entry(port).or_insert(targets);
            }
        }
        let defaults: Vec<(String, f32)> = def
            .defaults
            .iter()
            .filter(|(p, _)| def.port_slot(p) == slot.map(|(_, name)| name))
            .map(|(p, v)| (p.clone(), *v))
            .collect();
        match slot {
            Some((instance, name)) => {
                self.graph_voices.insert(
                    group_id,
                    GraphVoice {
                        instance,
                        slot: name.to_string(),
                        surface,
                        children: child_of,
                    },
                );
                if let Some(inst) = self.graph_instances.get_mut(&instance) {
                    inst.voices.insert(group_id);
                }
            }
            None => {
                self.graph_instances.insert(
                    group_id,
                    GraphInstance {
                        def,
                        shared_nodes: node_of,
                        bus_index,
                        audio_buses,
                        control_buses,
                        surface,
                        children: child_of,
                        voices: HashSet::new(),
                    },
                );
            }
        }
        // **A def's own defaults are applied here**, so a nested graph arrives
        // configured the way its author wrote it. Whoever instantiated it
        // applies its overrides afterwards and wins, which is the order a
        // caller expects and the reason this is not the caller's job.
        for (port, value) in defaults {
            self.apply_surface(group_id, &port, value, cmds);
        }
        group_id
    }
}

impl CmdTranslator {
    /// `/graph_new name id action target [port value ...]`: instantiate a
    /// GraphDef as a group holding its **shared** members, with private buses
    /// and a resolved named surface. (Slot members wait for `/graph_addSlot`.)
    /// It expands entirely into existing primitives (a group, member
    /// `/synth_new`s, `/node_map` wiring), so the engine sees nothing new and
    /// RT-safety is untouched. Atomic over the whole nested tree: everything
    /// fallible happens in the plan, before any command or mirror change.
    pub(in crate::osc::translate) fn graph_new(
        &mut self,
        msg: &rosc::OscMessage,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
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
        let action = AddAction::from_i32(*action).ok_or("add action must be 0-4")?;
        if *id != -1 && *id <= 0 {
            return Err("group ID must be positive or -1".into());
        }
        let plan = self.plan_instance(name, *id, None, &HashMap::new(), 0)?;
        let def = Arc::clone(&plan.def);
        let group_id = self.realize(plan, *target, action, None, cmds);
        // The def's own defaults were applied as it was built; these are the
        // caller's overrides on top of them.
        for (port, value) in Self::port_overrides(rest) {
            self.apply_surface(group_id, &port, value, cmds);
        }
        let _ = def;
        Ok(())
    }

    /// `/graph_addSlot instanceID slot id [port value ...]`: build one more of
    /// a named slot inside a running GraphDef instance, wired to its shared
    /// private buses — a voice of a synth, a clip on a track, an effect in a
    /// chain.
    ///
    /// The slot is a sub-group at the head of the instance group (the auto-sort
    /// then orders it relative to the shared mixer by its bus usage); freeing it
    /// (`/node_free`) frees its members and anything nested in them. Same atomic
    /// shape as `/graph_new`.
    pub(in crate::osc::translate) fn graph_add_slot(
        &mut self,
        instance: i32,
        slot: &str,
        id: i32,
        rest: &[OscType],
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let instance = self.slot_instance(instance)?;
        let Some(inst) = self.graph_instances.get(&instance) else {
            return Err(format!("GraphDef instance {instance} not found"));
        };
        let def = Arc::clone(&inst.def);
        let external = inst.bus_index.clone();
        if !def.has_slot(slot) {
            return Err(format!("GraphDef has no '{slot}' slot"));
        }
        if id != -1 && id <= 0 {
            return Err("slot ID must be positive or -1".into());
        }
        // The slot's members are built against the instance's buses, which is
        // what "wired to the same private buses" means: every one of them is
        // external as far as this sub-graph is concerned.
        let plan = self.plan_instance(&def.name, id, Some(slot), &external, 0)?;
        let slot_id = self.realize(
            plan,
            instance,
            AddAction::Head,
            Some((instance, slot)),
            cmds,
        );

        // The slot's own defaults were applied as it was built; these are the
        // caller's overrides on top of them.
        for (port, value) in Self::port_overrides(rest) {
            self.apply_surface(slot_id, &port, value, cmds);
        }
        Ok(())
    }

    /// `/graph_newVoice instanceID id [port value ...]`: the voice slot's own
    /// spelling of [`CmdTranslator::graph_add_slot`], kept because a voice is
    /// what slots were before they had names, and because MIDI notes spawn one.
    pub(in crate::osc::translate) fn graph_voice(
        &mut self,
        msg: &rosc::OscMessage,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let [OscType::Int(instance), OscType::Int(id), rest @ ..] = msg.args.as_slice() else {
            return Err("expected: instanceID, voiceID [, port, value ...]".into());
        };
        self.graph_add_slot(*instance, VOICE_SLOT, *id, rest, cmds)
    }

    /// The instance an id means when a slot is added to it.
    ///
    /// A slot group that *is* one nested graph — a track inside a piece, a clip
    /// inside a track — stands for that graph, because that is what the caller
    /// was handed and what everything else about it already answers to. Any
    /// other id is itself.
    fn slot_instance(&self, id: i32) -> Result<i32, String> {
        let Some(voice) = self.graph_voices.get(&id) else {
            return Ok(id);
        };
        match voice.children.values().copied().collect::<Vec<_>>()[..] {
            [only] => Ok(only),
            [] => Err(format!(
                "{id} fills the '{}' slot with nodes, not with a graph, so it has no slots of its own",
                voice.slot
            )),
            _ => Err(format!(
                "{id} fills the '{}' slot with several graphs; name the one to add to",
                voice.slot
            )),
        }
    }

    /// `/graph_addSlot instanceID slot id [port value ...]` off the wire.
    pub(in crate::osc::translate) fn graph_slot(
        &mut self,
        msg: &rosc::OscMessage,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let [
            OscType::Int(instance),
            OscType::String(slot),
            OscType::Int(id),
            rest @ ..,
        ] = msg.args.as_slice()
        else {
            return Err("expected: instanceID, slot, id [, port, value ...]".into());
        };
        self.graph_add_slot(*instance, slot, *id, rest, cmds)
    }

    /// Writes a surface-port value to its resolved member controls, scaled per
    /// target (`mul`·v + `add`), mirroring each write and re-sorting if a
    /// target turns out to be a bus-index control. `group` may be an instance
    /// (shared surface) or a voice sub-group (voice surface).
    /// `/graph_map instanceID port bus [audio]`: **drive a port from a bus**
    /// instead of from a value.
    ///
    /// The other half of `/node_set` against a surface, and the one an
    /// automation needs: a curve is a node writing a control bus, and what it
    /// drives is a port — of a track, of a clip, of an effect three levels down
    /// — whose member ids are private and are meant to stay that way. Without
    /// this a client would have to be told the node behind a port, which is the
    /// encapsulation the surface exists to keep.
    ///
    /// A negative bus **unmaps**, which is how a curve that was switched off
    /// gives the port back to whoever sets it by hand. The port's own scaling
    /// (`mul`/`add`) is not applied: a bus carries a signal and scaling it would
    /// need a node, so a port with a scaled target maps the bus straight onto
    /// the control and the scaling belongs to whatever wrote the bus.
    pub(in crate::osc::translate) fn graph_map(
        &mut self,
        msg: &rosc::OscMessage,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let [
            OscType::Int(instance),
            OscType::String(port),
            OscType::Int(bus),
            rest @ ..,
        ] = msg.args.as_slice()
        else {
            return Err("expected: instanceID, port, bus [, audio]".into());
        };
        let audio = matches!(rest.first(), Some(OscType::Int(1)));
        let targets = self
            .graph_instances
            .get(instance)
            .and_then(|inst| inst.surface.get(port.as_str()))
            .or_else(|| {
                self.graph_voices
                    .get(instance)
                    .and_then(|v| v.surface.get(port.as_str()))
            });
        let Some(targets) = targets.cloned() else {
            return Err(format!("{instance} has no port '{port}'"));
        };
        for (node, index, _mul, _add) in targets {
            cmds.push(Cmd::MapControl {
                id: node,
                index,
                bus: *bus,
                audio,
            });
            if self.mirror.set_map(node, index, *bus, audio) {
                self.reanalyze_and_resort(node, cmds);
            }
        }
        Ok(())
    }

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
    /// false so `/node_set` falls back to the synth/group path.
    pub(in crate::osc::translate) fn graph_set(
        &mut self,
        id: i32,
        pairs: &[OscType],
        cmds: &mut Vec<Cmd>,
    ) -> bool {
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

    /// Drops the translator-side state of a freed GraphDef node: a slot
    /// sub-group (forget it, detach from its instance) or an instance group
    /// (reclaim its private buses and forget its slots). A no-op for ordinary
    /// nodes. The actual node teardown is the `/node_free` `FreeNode` itself.
    ///
    /// **Recursive, because instancing is.** A nested graph is a registered
    /// instance of its own with buses of its own, and freeing the group it sits
    /// in frees the nodes but not the bookkeeping — so the children are walked
    /// here. External buses are never released: they were the parent's, and the
    /// child only ever borrowed the index.
    pub(in crate::osc::translate) fn free_graph_node(&mut self, id: i32) {
        if let Some(voice) = self.graph_voices.remove(&id) {
            if let Some(inst) = self.graph_instances.get_mut(&voice.instance) {
                inst.voices.remove(&id);
            }
            for (_, child) in voice.children {
                self.free_graph_node(child);
            }
            return;
        }
        if let Some(inst) = self.graph_instances.remove(&id) {
            for v in inst.voices {
                self.free_graph_node(v);
            }
            for (_, child) in inst.children {
                self.free_graph_node(child);
            }
            // A refused release here would mean the instance lost track of a
            // bus — surface it, never absorb it.
            for (first, width) in inst.audio_buses {
                if self.graph_audio_buses.release(first as i64, width).is_err() {
                    tracing::warn!("graph instance {id} released untracked audio bus {first}");
                }
            }
            for (first, width) in inst.control_buses {
                if self
                    .graph_control_buses
                    .release(first as i64, width)
                    .is_err()
                {
                    tracing::warn!("graph instance {id} released untracked control bus {first}");
                }
            }
        }
    }
}
