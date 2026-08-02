//! Instancing a GraphDef: `/graph_new` and `/graph_newVoice`.
//!
//! A GraphDef is a wiring, not a sound: the spec names member defs, the
//! private buses between them and the surface ports a client sets. Building
//! one means allocating those buses and the members' node ids, instantiating
//! each member with its wiring baked into the reserved `out`/`in` controls,
//! and resolving the surface into the (node, control) pairs a port drives.
//!
//! Every allocation here is all or nothing: a shortfall anywhere hands back
//! every id and every bus run it took, so a rejected instantiation leaves the
//! pools exactly as it found them.

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
    fn alloc_graph_buses(&mut self, def: &GraphDefSpec) -> Result<GraphBusAlloc, String> {
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

    /// `/graph_new name id action target [port value ...]`: instantiate a
    /// GraphDef as a group holding its **shared** members, with private buses
    /// and a resolved named surface. (Per-voice members wait for
    /// `/graph_newVoice`.) It expands entirely into existing primitives (a group,
    /// member `/synth_new`s, `/node_map` wiring), so the engine sees nothing new and
    /// RT-safety is untouched. Atomic: every fallible step (member def
    /// resolution, bus allocation) happens before any command or mirror change.
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
        let def = self
            .graph_defs
            .get(name)
            .cloned()
            .ok_or_else(|| format!("GraphDef not found: {name}"))?;
        let action = AddAction::from_i32(*action).ok_or("add action must be 0-4")?;
        if *id != -1 && *id <= 0 {
            return Err("group ID must be positive or -1".into());
        }

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
        // Ids last, so an id shortfall only has buses to hand back (and a bus
        // shortfall never touched the id registry).
        let ids = match self.alloc_graph_ids(*id, shared.len()) {
            Ok(ids) => ids,
            Err(e) => {
                self.free_graph_buses(&audio_buses, &control_buses);
                return Err(e);
            }
        };
        let (group_id, member_ids) = ids;

        // --- infallible phase: build the instance. The instance group is
        // auto-sorted so member (and voice sub-group) order follows the bus
        // connections; manual ordering is the graph's, not the client's.
        cmds.push(Cmd::AddGroup {
            id: group_id,
            target: *target,
            action,
            group: self.new_group(),
        });
        let _ = self
            .mirror
            .insert(group_id, MirrorBody::group(true), *target, action);
        let shared_nodes =
            self.build_members(&def, &shared, built, member_ids, group_id, &bus_index, cmds);
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

    /// `/graph_newVoice instanceID id [port value ...]`: spawn a per-voice
    /// sub-graph inside a running GraphDef instance, wired to its shared
    /// private buses. The voice is a sub-group at the head of the instance
    /// group (the auto-sort then orders it relative to the shared mixer by its
    /// bus usage); freeing it (`/node_free`) frees its members. Same atomic
    /// shape as `/graph_new`.
    pub(in crate::osc::translate) fn graph_voice(
        &mut self,
        msg: &rosc::OscMessage,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
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
        if *id != -1 && *id <= 0 {
            return Err("voice ID must be positive or -1".into());
        }
        // fallible: build the voice's synths before touching anything.
        let mut built = Vec::with_capacity(voice_indices.len());
        for &mi in &voice_indices {
            built.push(self.make_synth(&def.members[mi].def)?);
        }
        let (voice_id, member_ids) = self.alloc_graph_ids(*id, voice_indices.len())?;
        // infallible: the voice sub-group, into the instance group.
        cmds.push(Cmd::AddGroup {
            id: voice_id,
            target: *instance,
            action: AddAction::Head,
            group: self.new_group(),
        });
        let _ = self.mirror.insert(
            voice_id,
            MirrorBody::group(true),
            *instance,
            AddAction::Head,
        );
        let voice_nodes = self.build_members(
            &def,
            &voice_indices,
            built,
            member_ids,
            voice_id,
            &bus_index,
            cmds,
        );
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

    /// Drops the translator-side state of a freed GraphDef node: a voice
    /// sub-group (forget it, detach from its instance) or an instance group
    /// (reclaim its private buses and forget its voices). A no-op for ordinary
    /// nodes. The actual node teardown is the `/node_free` `FreeNode` itself.
    pub(in crate::osc::translate) fn free_graph_node(&mut self, id: i32) {
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
