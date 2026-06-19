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

use std::collections::HashMap;
use std::sync::Arc;

use rosc::OscType;

use crate::dsp::buffer::{Buffer, BufferPool, NUM_BUFFERS};
#[cfg(feature = "faust")]
use crate::faust::synth::{FaustDef, FaustSynth};
use crate::midi::{ChannelVoiceMessage, MidiBinding, MidiBindings, convert};
use crate::node::{AddAction, Group, Place, SynthNode};
use crate::osc::graph::{BusUsage, MirrorBody, TreeMirror, ugen_usage};
use crate::server::engine::Cmd;
use crate::server::nrt::NrtJob;
use crate::synthdef::instance::UGenSynth;
use crate::synthdef::{SynthDef, SynthDefSpec, compile, default_spec};

#[cfg(feature = "faust")]
use crate::osc::graph::faust_usage;

/// Auto-assigned node IDs (`/s_new` with ID -1) start above this.
const AUTO_NODE_ID_BASE: i32 = 2_000_000;

/// What a live node was built from, mirrored per node ID so `/n_set` can
/// resolve control names off the audio thread.
#[derive(Clone)]
pub enum NodeDef {
    UGen(Arc<SynthDef>),
    #[cfg(feature = "faust")]
    Faust(Arc<FaustDef>),
}

impl NodeDef {
    pub fn control_index(&self, name: &str) -> Option<u32> {
        match self {
            NodeDef::UGen(def) => def.control_index(name),
            #[cfg(feature = "faust")]
            NodeDef::Faust(def) => def.control_index(name),
        }
    }

    /// Control name by index, for `/g_queryTree.reply`.
    pub fn control_name(&self, index: usize) -> Option<&str> {
        match self {
            NodeDef::UGen(def) => def.control_names.get(index).map(String::as_str),
            #[cfg(feature = "faust")]
            NodeDef::Faust(def) => match index.checked_sub(def.params.len()) {
                None => def.params.get(index).map(|p| p.name.as_str()),
                Some(0) => Some("out"),
                Some(1) => Some("in"),
                Some(_) => None,
            },
        }
    }

    /// Default control values of a fresh instance.
    fn control_defaults(&self) -> Vec<f32> {
        match self {
            NodeDef::UGen(def) => def.control_defaults.clone(),
            #[cfg(feature = "faust")]
            NodeDef::Faust(def) => {
                // UI params at their inits, then the reserved out/in buses.
                let mut v: Vec<f32> = def.params.iter().map(|p| p.init).collect();
                v.extend([0.0, 0.0]);
                v
            }
        }
    }

    /// Bus usage of an instance with these control values (M12).
    fn usage(&self, controls: &[f32]) -> (BusUsage, Vec<u32>) {
        match self {
            NodeDef::UGen(def) => ugen_usage(def, controls),
            #[cfg(feature = "faust")]
            NodeDef::Faust(def) => faust_usage(def, controls),
        }
    }
}

pub struct CmdTranslator {
    /// Faust instances bake the sample rate in at `/s_new` time.
    #[cfg_attr(not(feature = "faust"), allow(dead_code))]
    sample_rate: f32,
    /// Loaded SynthDefs; starts with the built-in "default".
    pub defs: HashMap<String, Arc<SynthDef>>,
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
    /// M17: per-channel MIDI bindings and the live voice table. Channel-voice
    /// messages actuate nodes through [`Self::translate_midi`].
    pub midi: MidiBindings,
}

impl CmdTranslator {
    pub fn new(sample_rate: f32) -> Self {
        let mut defs = HashMap::new();
        let default = compile(default_spec()).expect("built-in default def must compile");
        defs.insert(default.name.clone(), Arc::new(default));
        Self {
            sample_rate,
            defs,
            node_defs: HashMap::new(),
            next_auto_id: AUTO_NODE_ID_BASE,
            #[cfg(feature = "faust")]
            faust_defs: HashMap::new(),
            mirror: TreeMirror::new(),
            midi: MidiBindings::new(),
        }
    }

    /// Total defs of both families, for `/status.reply`.
    pub fn def_count(&self) -> usize {
        #[allow(unused_mut)]
        let mut n = self.defs.len();
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
        if let Some(def) = self.defs.get(name) {
            let synth = Box::new(UGenSynth::new(Arc::clone(def)));
            return Ok((synth, NodeDef::UGen(Arc::clone(def))));
        }
        #[cfg(feature = "faust")]
        if let Some(def) = self.faust_defs.get(name) {
            let synth = FaustSynth::new(Arc::clone(def), self.sample_rate)?;
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

    /// `/n_map` (control bus) and `/n_mapa` (audio bus): bind controls to
    /// buses the synth reads at the start of every block, `bus = -1` to
    /// unbind. Same pair-wise parsing as `/n_set`; an audio map (or mapping a
    /// control used as a bus index) re-analyzes the node's bus usage.
    fn map_controls(
        &mut self,
        msg: &rosc::OscMessage,
        audio: bool,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let Some(OscType::Int(id)) = msg.args.first() else {
            return Err("expected: id, then control/bus pairs".into());
        };
        let def = self
            .node_defs
            .get(id)
            .cloned()
            .ok_or_else(|| format!("node {id} not found"))?;
        let mut usage_hit = false;
        for pair in msg.args[1..].chunks(2) {
            if let (Some(index), Some(bus)) =
                (control_key(&pair[0], &def), pair.get(1).and_then(int_value))
            {
                cmds.push(Cmd::MapControl {
                    id: *id,
                    index,
                    bus,
                    audio,
                });
                usage_hit |= self.mirror.set_map(*id, index, bus, audio);
            }
        }
        if usage_hit {
            self.reanalyze_and_resort(*id, cmds);
        }
        Ok(())
    }

    /// `/d_recv`: compile a SynthDef JSON blob into the def table. Returns the
    /// def name, so the caller can persist the spec under it.
    pub fn d_recv(&mut self, args: &[OscType]) -> Result<String, String> {
        let bytes: &[u8] = match args.first() {
            Some(OscType::Blob(b)) => b,
            Some(OscType::String(s)) => s.as_bytes(),
            _ => return Err("expected a JSON blob or string".into()),
        };
        let spec: SynthDefSpec =
            serde_json::from_slice(bytes).map_err(|e| format!("invalid JSON: {e}"))?;
        let def = compile(spec)?;
        let name = def.name.clone();
        self.defs.insert(name.clone(), Arc::new(def));
        Ok(name)
    }

    /// `/d_free name...`. Live synths keep their `Arc<SynthDef>`: scsynth
    /// semantics. Same for Faust factories (instances refcount them).
    pub fn d_free(&mut self, args: &[OscType]) -> Result<(), String> {
        for arg in args {
            let OscType::String(name) = arg else {
                return Err("expected synthdef names".into());
            };
            self.defs.remove(name);
            #[cfg(feature = "faust")]
            self.faust_defs.remove(name);
        }
        Ok(())
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
                let def = self
                    .node_defs
                    .get(id)
                    .cloned()
                    .ok_or_else(|| format!("node {id} not found"))?;
                let mut bus_control_hit = false;
                for pair in msg.args[1..].chunks(2) {
                    if let (Some(index), Some(value)) = (
                        control_key(&pair[0], &def),
                        pair.get(1).and_then(float_value),
                    ) {
                        cmds.push(Cmd::SetControl {
                            id: *id,
                            index,
                            value,
                        });
                        bus_control_hit |= self.mirror.set_control(*id, index, value);
                        // An explicit set clears any mapping on that control
                        // (scsynth); dropping an audio map changes usage.
                        bus_control_hit |= self.mirror.set_map(*id, index, -1, false);
                    }
                }
                if bus_control_hit {
                    self.reanalyze_and_resort(*id, cmds);
                }
                Ok(())
            }
            "/n_map" => self.map_controls(msg, false, cmds),
            "/n_mapa" => self.map_controls(msg, true, cmds),
            // M17 MIDI binding config (no engine command; pure translator state).
            "/midi_bind" => self.midi_bind(msg),
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
                        group: Group::new(),
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
            other => Err(format!("{other} cannot be scheduled in a timed bundle")),
        }
    }

    /// `/midi_bind channel instrument [target] [addAction] [gate]`: bind a MIDI
    /// channel to an instrument def (SynthDef *or* FaustDef). Default control
    /// map is `freq`/`amp`; `/midi_map` extends it.
    fn midi_bind(&mut self, msg: &rosc::OscMessage) -> Result<(), String> {
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
        self.midi.channels.insert(
            channel,
            MidiBinding::new(instrument.clone(), target, action, gate),
        );
        Ok(())
    }

    /// `/midi_unbind channel`: drop the binding and free every voice still
    /// sounding on that channel.
    fn midi_unbind(&mut self, msg: &rosc::OscMessage, cmds: &mut Vec<Cmd>) -> Result<(), String> {
        let Some(OscType::Int(channel)) = msg.args.first() else {
            return Err("expected: channel".into());
        };
        let channel = midi_channel(*channel)?;
        self.midi.channels.remove(&channel);
        for id in self.midi.drain_channel(channel) {
            cmds.push(Cmd::FreeNode { id });
            self.mirror.remove(id);
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
        if self.midi.voices.contains_key(&(channel, note)) {
            self.midi_note_off(channel, note, cmds)?;
        }
        let id = self.midi.alloc_id();
        let s_new = midi_message(
            "/s_new",
            vec![
                OscType::String(instrument),
                OscType::Int(id),
                OscType::Int(action),
                OscType::Int(target),
                OscType::String(freq_control),
                OscType::Float(convert::midi2freq(note as f32)),
                OscType::String(amp_control),
                OscType::Float(convert::velocity2amp(velocity)),
            ],
        );
        self.translate(&s_new, cmds)?;
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
    if index < 0 || index as usize >= NUM_BUFFERS {
        return Err(format!("buffer index out of range: {index}"));
    }
    Ok((index, job))
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
