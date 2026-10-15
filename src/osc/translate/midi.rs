//! MIDI: what a channel is routed to, and the events it writes.
//!
//! **A binding is routing.** `/midi_bind channel bus` sends the channel's
//! messages to an event bus, where whatever reads it plays them -- a `Voices`
//! node, a plugin, any node whose `notes` control names the bus. The
//! instrument form, `/midi_bind channel def`, is that routing with a node of
//! its own: a `Voices` node of a SynthDef or a FaustDef, or a plugin's node,
//! made on a bus of the binding's and freed with it, its voice map the
//! binding's control map (`/midi_map`). Either way a note on the channel
//! makes no node.
//!
//! An MPE zone is routed the same way (`/midi_bindZone`): the zones' decoder
//! gives each note its own identity, and its bend, pressure and timbre reach
//! that note alone as its expression.
//!
//! A GraphDef is the exception: its notes are its voice slots, so a channel
//! bound to one spawns a `/graph_newVoice` per note into the graph's shared
//! instance, built through [`CmdTranslator::translate`] as the same message
//! from a client would be.

use clausters_core::event_bus::{BusEvent, Expression, Kind, midi_source};
use clausters_midi::mpe::{MpeEvent, Side};

use super::*;
use crate::midi::{ZoneBinding, ZoneVoice, to_bus_event};
use crate::osc::args::Args;

/// Voices a binding's `Voices` node holds.
pub const BINDING_VOICES: i32 = 16;

/// Where a channel's messages go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiRoute {
    /// An event bus.
    Bus(u32),
    /// A GraphDef's shared instance, a voice slot per note.
    Graph,
}

/// The source a zone's notes carry: the MIDI input's, past its channels.
fn zone_source(master: u8) -> u128 {
    midi_source(256 + u16::from(master))
}

impl CmdTranslator {
    /// `/midi_bind channel bus` routes MIDI `channel` to event bus `bus`;
    /// `/midi_bind channel instrument [target] [addAction] [gate]` makes a
    /// node of `instrument` on a bus of the binding's and routes the channel
    /// there: a `Voices` node of a SynthDef or a FaustDef, or a plugin's
    /// node; a GraphDef's shared instance, whose voice slots the notes fill.
    /// A binding the channel had is unbound first.
    pub(in crate::osc::translate) fn midi_bind(
        &mut self,
        msg: &rosc::OscMessage,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let (channel, binding) = match msg.args.as_slice() {
            [OscType::Int(channel), OscType::Int(bus)] => (
                midi_channel(*channel)?,
                MidiBinding::routed(self.event_bus(*bus)?),
            ),
            [OscType::Int(channel), OscType::String(instrument), ..] => {
                let mut tail = Args::after(&msg.args, 2);
                let target = tail.opt_int()?.unwrap_or(0);
                let action = tail.opt_int()?.unwrap_or(0);
                let gate = tail.opt_int()?.unwrap_or(0) != 0;
                if AddAction::from_i32(action).is_none() {
                    return Err("add action must be 0-4".into());
                }
                (
                    midi_channel(*channel)?,
                    MidiBinding::new(instrument.clone(), target, action, gate),
                )
            }
            _ => {
                return Err(
                    "expected: channel, then an event bus or an instrument [, target, addAction, gate]"
                        .into(),
                );
            }
        };
        if channel < 16 && self.midi.decoder.zone_of(channel).is_some() {
            return Err(format!(
                "channel {channel} is in an MPE zone: unbind the zone first"
            ));
        }
        let mut binding = binding;
        self.bind_instrument(&mut binding, cmds)?;
        if let Some(old) = self.midi.channels.insert(channel, binding) {
            let voices = self.midi.drain_channel(channel);
            self.unbind_resources(old, voices, cmds);
        }
        self.midi.refresh_allowed();
        Ok(())
    }

    /// An event bus a client named, checked against the server's.
    fn event_bus(&self, bus: i32) -> Result<u32, String> {
        u32::try_from(bus)
            .ok()
            .filter(|b| (*b as usize) < self.limits.event_buses)
            .ok_or_else(|| format!("event bus must be in range 0..{}", self.limits.event_buses))
    }

    /// `/midi_bindZone master members bus` routes an **MPE zone** to event
    /// bus `bus`; `/midi_bindZone master members instrument [target]
    /// [addAction] [gate]` makes its node, as `/midi_bind` does. `master` is
    /// channel 0 (the lower zone, members ascending from 1) or 15 (the upper,
    /// descending from 14); `members` is how many (0 waits for the device's
    /// RPN 6, which wins whenever it comes). Each note has its own identity,
    /// and a member's bend, pressure and timbre reach that note only -- the
    /// bend as its tuning, the member and master bends summed through their
    /// ranges. A zone whose channels reach one a per-channel binding holds is
    /// refused.
    pub(in crate::osc::translate) fn midi_bind_zone(
        &mut self,
        msg: &rosc::OscMessage,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let [OscType::Int(master), OscType::Int(members), what, ..] = msg.args.as_slice() else {
            return Err(
                "expected: master, members, then an event bus or an instrument [, target, addAction, gate]"
                    .into(),
            );
        };
        let side = u8::try_from(*master)
            .ok()
            .and_then(Side::of_master)
            .ok_or("a zone's master is channel 0 (lower) or 15 (upper)")?;
        let members = u8::try_from(*members)
            .ok()
            .filter(|m| *m <= 15)
            .ok_or("a zone has 0-15 members")?;
        let mut zone = match what {
            OscType::Int(bus) => ZoneBinding::routed(members, self.event_bus(*bus)?),
            OscType::String(instrument) => {
                let mut tail = Args::after(&msg.args, 3);
                let target = tail.opt_int()?.unwrap_or(0);
                let action = tail.opt_int()?.unwrap_or(0);
                let gate = tail.opt_int()?.unwrap_or(0) != 0;
                if AddAction::from_i32(action).is_none() {
                    return Err("add action must be 0-4".into());
                }
                ZoneBinding::new(members, instrument.clone(), target, action, gate)
            }
            _ => return Err("a zone is routed to an event bus or bound to an instrument".into()),
        };
        self.bind_zone(side, &mut zone, cmds)?;
        self.midi.zones.insert(side.master(), zone);
        Ok(())
    }

    /// Puts `zone` on `side`: refused over a bound channel, its node made,
    /// the decoder told. A zone already there is unbound first. Shared by
    /// `/midi_bindZone` and the restore.
    fn bind_zone(
        &mut self,
        side: Side,
        zone: &mut ZoneBinding,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        #[cfg(all(
            any(feature = "plugin-clap", feature = "plugin-vst3"),
            target_os = "linux"
        ))]
        if self
            .plugin_defs
            .get(&zone.binding.instrument)
            .is_some_and(|def| {
                def.info
                    .note_inputs
                    .first()
                    .is_none_or(|p| p.dialects & 1 == 0)
            })
        {
            return Err("MPE bindings require a CLAP note-expression port".into());
        }

        let reach: Vec<u8> = std::iter::once(side.master())
            .chain(side.members(zone.config.members))
            .collect();
        if let Some(c) = reach.iter().find(|c| self.midi.channels.contains_key(c)) {
            return Err(format!(
                "the zone would reach channel {c}, which a /midi_bind holds"
            ));
        }
        if self.midi.zones.contains_key(&side.master()) {
            self.unbind_zone(side.master(), cmds);
        }
        self.bind_instrument(&mut zone.binding, cmds)?;
        self.midi.decoder.set_zone(side, Some(zone.config.members));
        self.midi.decoder.set_timbre_cc(side, zone.config.timbre_cc);
        // What the zone's notes end on is freed with them below, not replayed.
        while self.midi.decoder.poll().is_some() {}
        self.midi.refresh_allowed();
        Ok(())
    }

    /// Unbinds the zone on `master`: its node (or its graph voices) freed and
    /// the decoder told.
    fn unbind_zone(&mut self, master: u8, cmds: &mut Vec<Cmd>) {
        let Some(zone) = self.midi.zones.remove(&master) else {
            return;
        };
        if let Some(side) = Side::of_master(master) {
            self.midi.decoder.set_zone(side, None);
            while self.midi.decoder.poll().is_some() {}
        }
        let notes: Vec<u32> = self
            .midi
            .zone_voices
            .iter()
            .filter(|(_, v)| v.master == master)
            .map(|(note, _)| *note)
            .collect();
        let voices: Vec<i32> = notes
            .iter()
            .filter_map(|n| self.midi.zone_voices.remove(n).map(|v| v.id))
            .collect();
        self.unbind_resources(zone.binding, voices, cmds);
    }

    /// Makes what an instrument binding plays: a node on a bus of the
    /// binding's -- a plugin's, or a `Voices` node with the binding's map --
    /// or a GraphDef's shared instance. A routed binding makes nothing.
    fn bind_instrument(
        &mut self,
        binding: &mut MidiBinding,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        if binding.route.is_some() {
            binding.bus = binding.route;
            return Ok(());
        }
        let instrument = binding.instrument.clone();
        if self.graph_defs.contains_key(&instrument) {
            binding.graph_instance =
                self.bind_graph_instance(&instrument, binding.target, binding.action, cmds)?;
            return Ok(());
        }
        let bus = self
            .binding_buses
            .alloc(1)
            .ok_or("out of event buses for MIDI bindings: unbind one")? as u32;
        let id = match self.midi.alloc_id() {
            Some(id) => id,
            None => {
                let _ = self.binding_buses.release(i64::from(bus), 1);
                return Err("out of MIDI node ids: ids recycle when their nodes end".into());
            }
        };
        let made = self.binding_node(binding, id, bus, cmds);
        if let Err(why) = made {
            let _ = self.binding_buses.release(i64::from(bus), 1);
            self.midi.release_id(i64::from(id));
            return Err(why);
        }
        binding.node = Some(id);
        binding.bus = Some(bus);
        Ok(())
    }

    /// Node `id` of an instrument binding, reading event bus `bus`.
    fn binding_node(
        &mut self,
        binding: &MidiBinding,
        id: i32,
        bus: u32,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let place = [
            OscType::Int(id),
            OscType::Int(binding.action),
            OscType::Int(binding.target),
        ];
        let notes = [OscType::String("notes".into()), OscType::Float(bus as f32)];
        #[cfg(all(
            any(feature = "plugin-clap", feature = "plugin-vst3"),
            target_os = "linux"
        ))]
        if self.plugin_defs.contains_key(&binding.instrument) {
            let mut args = vec![OscType::String(binding.instrument.clone())];
            args.extend(place);
            args.extend(notes);
            return self.translate(&midi_message("/synth_new", args), cmds);
        }
        let mut args = vec![OscType::String(binding.instrument.clone())];
        args.extend(place);
        args.push(OscType::Int(BINDING_VOICES));
        args.extend(notes);
        self.translate(&midi_message("/voices_new", args), cmds)
            .map_err(|why| match why.starts_with("SynthDef not found") {
                true => format!("instrument not found: {}", binding.instrument),
                false => why,
            })?;
        self.binding_map(binding, id, cmds);
        Ok(())
    }

    /// The binding's control map, as the voice map of its `Voices` node: what
    /// it names, where the def has it. A name the def lacks maps nothing.
    fn binding_map(&mut self, b: &MidiBinding, id: i32, cmds: &mut Vec<Cmd>) {
        let mut entries: Vec<(String, String)> = vec![
            ("note".into(), b.freq_control.clone()),
            ("vel".into(), b.amp_control.clone()),
            ("gate".into(), b.gate_control.clone()),
        ];
        for (selector, name) in [
            ("bend", &b.bend_control),
            ("pressure", &b.pressure_control),
            ("poly", &b.poly_control),
            ("timbre", &b.timbre_control),
            ("lift", &b.lift_control),
        ] {
            entries.push((selector.into(), name.clone().unwrap_or_default()));
        }
        for (n, name) in &b.cc {
            entries.push((format!("cc{n}"), name.clone()));
        }
        for (selector, name) in entries {
            if self.voice_map_cmd(id, &selector, &name, cmds).is_err() {
                // The def does not have it: the entry maps nothing.
                let _ = self.voice_map_cmd(id, &selector, "", cmds);
            }
        }
    }

    /// Frees what a binding made: its node and bus, or its GraphDef instance
    /// (which frees the voices with it), else the graph voices still sounding.
    fn unbind_resources(&mut self, binding: MidiBinding, voices: Vec<i32>, cmds: &mut Vec<Cmd>) {
        if let Some(node) = binding.node {
            cmds.push(Cmd::FreeNode { id: node });
            self.mirror.remove(node);
        }
        if binding.route.is_none()
            && let Some(bus) = binding.bus
        {
            let _ = self.binding_buses.release(i64::from(bus), 1);
        }
        if let Some(instance) = binding.graph_instance {
            cmds.push(Cmd::FreeNode { id: instance });
            self.mirror.remove(instance);
            self.free_graph_node(instance);
        } else {
            for id in voices {
                cmds.push(Cmd::FreeNode { id });
                self.mirror.remove(id);
            }
        }
    }

    /// If `instrument` names a GraphDef, spawn its shared instance now (so each
    /// note spawns a voice into it) and return the instance id. A GraphDef
    /// with no per-voice members is rejected -- it has nothing to play per
    /// note.
    fn bind_graph_instance(
        &mut self,
        instrument: &str,
        target: i32,
        action: i32,
        cmds: &mut Vec<Cmd>,
    ) -> Result<Option<i32>, String> {
        if !self.graph_defs[instrument].has_voice_members() {
            return Err(format!(
                "GraphDef {instrument:?} has no per-voice members to bind to MIDI"
            ));
        }
        let instance = self
            .midi
            .alloc_id()
            .ok_or("out of MIDI voice ids: ids recycle when their nodes end")?;
        let new = midi_message(
            "/graph_new",
            vec![
                OscType::String(instrument.to_string()),
                OscType::Int(instance),
                OscType::Int(action),
                OscType::Int(target),
            ],
        );
        if let Err(e) = self.graph_new(&new, cmds) {
            self.midi.release_id(instance as i64);
            return Err(e);
        }
        Ok(Some(instance))
    }

    /// Re-establish a persisted binding at startup, making its node again.
    /// Mirrors `/midi_bind` but takes the stored config directly.
    pub fn restore_binding(
        &mut self,
        pb: crate::midi::PersistedBinding,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        if let Some(config) = pb.zone {
            let side = Side::of_master(pb.channel).ok_or("a zone's master is channel 0 or 15")?;
            let mut zone = ZoneBinding {
                binding: pb.binding,
                config,
            };
            self.bind_zone(side, &mut zone, cmds)?;
            self.midi.zones.insert(pb.channel, zone);
            return Ok(());
        }
        let mut binding = pb.binding;
        if let Some(bus) = binding.route {
            self.event_bus(bus as i32)?;
        }
        self.bind_instrument(&mut binding, cmds)?;
        self.midi.channels.insert(pb.channel, binding);
        self.midi.refresh_allowed();
        Ok(())
    }

    /// `/midi_unbind channel`: drop the binding, and free what it made.
    pub(in crate::osc::translate) fn midi_unbind(
        &mut self,
        msg: &rosc::OscMessage,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let Some(OscType::Int(channel)) = msg.args.first() else {
            return Err("expected: channel".into());
        };
        let channel = midi_channel(*channel)?;
        if self.midi.zones.contains_key(&channel) {
            self.unbind_zone(channel, cmds);
            self.midi.refresh_allowed();
            return Ok(());
        }
        let binding = self.midi.channels.remove(&channel);
        self.midi.refresh_allowed();
        let voices = self.midi.drain_channel(channel);
        if let Some(binding) = binding {
            self.unbind_resources(binding, voices, cmds);
        }
        Ok(())
    }

    /// `/midi_map channel selector name [cc]`: a control an instrument
    /// binding's node takes from a message type. Selectors: `note`, `vel`,
    /// `gate`, `bend`, `pressure` (channel aftertouch), `poly` (per-note
    /// aftertouch), `lift` (note-off velocity), `ccN` (control change), and on
    /// a zone's master `timbre` (its third dimension; a fourth argument sets
    /// the controller that carries it, 74 unless given). On a `Voices` node
    /// it is the node's voice map; on a plugin's node a controller, pressure
    /// or bend mapped to a control sets that control.
    pub(in crate::osc::translate) fn midi_map(
        &mut self,
        msg: &rosc::OscMessage,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let [
            OscType::Int(channel),
            OscType::String(selector),
            OscType::String(name),
            tail @ ..,
        ] = msg.args.as_slice()
        else {
            return Err("expected: channel, selector, name".into());
        };
        let channel = midi_channel(*channel)?;
        let zone = self.midi.zones.get(&channel);
        let Some(binding) = zone
            .map(|z| &z.binding)
            .or_else(|| self.midi.channels.get(&channel))
        else {
            return Err(format!("channel {channel} is not bound"));
        };
        if let Some(bus) = binding.route {
            return Err(format!(
                "channel {channel} is routed to event bus {bus}: map controls on the node that \
                 reads it (/voices_map)"
            ));
        }
        if selector.starts_with("prog") {
            return Err(
                "a binding plays one instrument: a program change is the node's to read".into(),
            );
        }
        if selector == "timbre" && zone.is_none() {
            return Err("timbre is a zone's third dimension: bind one with /midi_bindZone".into());
        }
        let node = binding.node;
        if let Some(node) = node.filter(|n| self.plugin_node(*n)) {
            if zone.is_some()
                || matches!(
                    selector.as_str(),
                    "note" | "vel" | "velocity" | "gate" | "lift" | "poly" | "timbre"
                )
            {
                return Err(
                    "plugin notes and MPE use native routing; def selector maps are unsupported"
                        .into(),
                );
            }
            if self.node_defs[&node].control_index(name).is_none() {
                return Err("unknown plugin control in MIDI map".into());
            }
        } else if let Some(node) = node {
            self.voice_map_cmd(node, selector, name, cmds)?;
        }
        let binding = match self.midi.zones.get_mut(&channel) {
            Some(zone) => {
                if selector == "timbre"
                    && let Some(OscType::Int(cc)) = tail.first()
                {
                    let cc = u8::try_from(*cc)
                        .ok()
                        .filter(|c| *c < 128)
                        .ok_or("a controller is 0-127")?;
                    zone.config.timbre_cc = cc;
                    if let Some(side) = Side::of_master(channel) {
                        self.midi.decoder.set_timbre_cc(side, cc);
                    }
                }
                &mut zone.binding
            }
            None => self
                .midi
                .channels
                .get_mut(&channel)
                .ok_or_else(|| format!("channel {channel} is not bound"))?,
        };
        match selector.as_str() {
            "note" => binding.freq_control = name.clone(),
            "vel" | "velocity" => binding.amp_control = name.clone(),
            "gate" => binding.gate_control = name.clone(),
            "bend" => binding.bend_control = Some(name.clone()),
            "pressure" => binding.pressure_control = Some(name.clone()),
            "poly" => binding.poly_control = Some(name.clone()),
            "timbre" => binding.timbre_control = Some(name.clone()),
            "lift" => binding.lift_control = Some(name.clone()),
            s if s.starts_with("cc") => {
                let n: u8 = s[2..].parse().map_err(|_| "bad cc selector".to_string())?;
                binding.cc.insert(n, name.clone());
            }
            other => return Err(format!("unknown MIDI selector {other:?}")),
        }
        Ok(())
    }

    /// A build with no def family has no `Voices` node to map.
    #[cfg(not(any(feature = "synth", feature = "faust")))]
    fn voice_map_cmd(
        &self,
        id: i32,
        _selector: &str,
        _control: &str,
        _cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        Err(format!("node {id} is not a Voices node"))
    }

    /// Whether node `id` is a plugin's.
    fn plugin_node(&self, id: i32) -> bool {
        #[cfg(all(
            any(feature = "plugin-clap", feature = "plugin-vst3"),
            target_os = "linux"
        ))]
        if matches!(self.node_defs.get(&id), Some(NodeDef::Plugin(..))) {
            return true;
        }
        let _ = id;
        false
    }

    /// Where `channel`'s messages go, when it is bound.
    pub(crate) fn midi_route(&self, channel: u8) -> Option<MidiRoute> {
        let b = self.midi.channels.get(&channel)?;
        match (b.bus, b.graph_instance) {
            (Some(bus), _) => Some(MidiRoute::Bus(bus)),
            (None, Some(_)) => Some(MidiRoute::Graph),
            (None, None) => None,
        }
    }

    /// Where the zone on `master` sends its notes, when there is one.
    pub(crate) fn zone_route(&self, master: u8) -> Option<MidiRoute> {
        let b = &self.midi.zones.get(&master)?.binding;
        match (b.bus, b.graph_instance) {
            (Some(bus), _) => Some(MidiRoute::Bus(bus)),
            (None, Some(_)) => Some(MidiRoute::Graph),
            (None, None) => None,
        }
    }

    /// **A raw MIDI message**, as the live input delivers it: through the
    /// zones' decoder, whose per-note messages are a zone's notes and whose
    /// untouched ones are a plain channel's ([`Self::translate_midi`]).
    /// Unbound channels and anything else that plays nothing are ignored.
    pub fn translate_midi_bytes(
        &mut self,
        bytes: &[u8],
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        // A zone's sustain pedal is the zone's whether or not a note sounds:
        // the decoder only passes a controller to the notes it reaches, so a
        // pedal lifted after the last note-off would never arrive.
        for chunk in bytes.as_chunks::<3>().0 {
            if chunk[0] & 0xf0 == 0xb0
                && chunk[1] == 64
                && let Some(side) = self.midi.decoder.zone_of(chunk[0] & 15)
                && let Some(msg) = crate::midi::parse_midi1(chunk[0], chunk[1], chunk[2])
            {
                self.zone_channel(side.master(), msg, cmds);
            }
        }
        self.midi.decoder.feed(bytes);
        let mut result = Ok(());
        while let Some(event) = self.midi.decoder.poll() {
            let done = match event {
                MpeEvent::Plain([status, d1, d2]) => match crate::midi::parse_midi1(status, d1, d2)
                {
                    Some(msg) => match self.midi.decoder.zone_of(status & 15) {
                        Some(side) => {
                            self.zone_channel(side.master(), msg, cmds);
                            Ok(())
                        }
                        None => self.translate_midi(msg, cmds),
                    },
                    None => Ok(()),
                },
                event => self.translate_mpe(event, cmds),
            };
            if done.is_err() && result.is_ok() {
                result = done;
            }
        }
        result
    }

    /// **A channel message inside a zone** the decoder passed through -- a
    /// controller, a program, the master's pedal: on a routed zone, the
    /// zone's, its sustain pedal for every note the zone holds.
    fn zone_channel(&mut self, master: u8, msg: ChannelVoiceMessage, cmds: &mut Vec<Cmd>) {
        if let Some(MidiRoute::Bus(bus)) = self.zone_route(master) {
            let mut event = to_bus_event(msg, zone_source(master));
            if event.kind == Kind::Controller && event.index == 64 {
                event.channel = -1;
            }
            cmds.push(Cmd::BusEvent { bus, event });
        }
    }

    /// **One per-note message of a zone**: on a routed zone, the events of
    /// the note it names (a note-on with its starting bend, pressure and
    /// timbre as its expression); on a GraphDef's, a voice slot per note.
    fn translate_mpe(&mut self, event: MpeEvent, cmds: &mut Vec<Cmd>) -> Result<(), String> {
        let master = match event {
            MpeEvent::NoteOn { side, .. } => side.master(),
            MpeEvent::NoteOff { note, .. }
            | MpeEvent::Bend { note, .. }
            | MpeEvent::Pressure { note, .. }
            | MpeEvent::Timbre { note, .. }
            | MpeEvent::Controller { note, .. } => match self.midi.zone_notes.get(&note) {
                Some(master) => *master,
                None => match self.midi.zone_voices.get(&note) {
                    Some(voice) => voice.master,
                    None => return Ok(()),
                },
            },
            MpeEvent::Plain(_) => return Ok(()),
        };
        match self.zone_route(master) {
            // The pedal went to the zone before the decoder saw it.
            Some(MidiRoute::Bus(_))
                if matches!(event, MpeEvent::Controller { controller: 64, .. }) =>
            {
                Ok(())
            }
            Some(MidiRoute::Bus(bus)) => {
                if let MpeEvent::NoteOn { note, .. } = event {
                    self.midi.zone_notes.insert(note, master);
                }
                if let MpeEvent::NoteOff { note, .. } = event {
                    self.midi.zone_notes.remove(&note);
                }
                for e in zone_events(event, zone_source(master), i16::from(master)) {
                    cmds.push(Cmd::BusEvent { bus, event: e });
                }
                Ok(())
            }
            Some(MidiRoute::Graph) => self.graph_mpe(event, cmds),
            None => Ok(()),
        }
    }

    /// A zone's note on a GraphDef binding: a voice slot per note, its
    /// expression as the voice's controls.
    fn graph_mpe(&mut self, event: MpeEvent, cmds: &mut Vec<Cmd>) -> Result<(), String> {
        match event {
            MpeEvent::NoteOn {
                note,
                side,
                key,
                velocity,
                bend,
                pressure,
                timbre,
                ..
            } => {
                let master = side.master();
                let id = self
                    .midi
                    .alloc_id()
                    .ok_or("out of MIDI voice ids: ids recycle when their nodes end")?;
                let Some(msg) = self.zone_voice(master, key, velocity, bend, pressure, timbre, id)
                else {
                    self.midi.release_id(id as i64);
                    return Ok(());
                };
                if let Err(e) = self.translate(&msg, cmds) {
                    self.midi.release_id(id as i64);
                    return Err(e);
                }
                self.midi
                    .zone_voices
                    .insert(note, ZoneVoice { id, key, master });
                Ok(())
            }
            MpeEvent::NoteOff { note, velocity } => {
                if let Some(voice) = self.midi.zone_voices.remove(&note) {
                    for msg in self.zone_release(voice, velocity) {
                        let _ = self.translate(&msg, cmds);
                    }
                }
                Ok(())
            }
            MpeEvent::Plain(_) => Ok(()),
            expression => {
                let note = match expression {
                    MpeEvent::Bend { note, .. }
                    | MpeEvent::Pressure { note, .. }
                    | MpeEvent::Timbre { note, .. }
                    | MpeEvent::Controller { note, .. } => note,
                    _ => return Ok(()),
                };
                if let Some(voice) = self.midi.zone_voices.get(&note).copied() {
                    for (control, value) in self.zone_expression(voice, expression) {
                        self.midi_set(voice.id, &control, value, cmds);
                    }
                }
                Ok(())
            }
        }
    }

    /// The message that starts graph voice `id` of the zone on `master`:
    /// `freq` at `key + bend` semitones, `amp` from the velocity, and the
    /// pressure and timbre controls the note starts with.
    #[allow(clippy::too_many_arguments)] // one note's whole state, as the decoder hands it
    fn zone_voice(
        &self,
        master: u8,
        key: u8,
        velocity: u16,
        bend: f32,
        pressure: u32,
        timbre: u32,
        id: i32,
    ) -> Option<rosc::OscMessage> {
        let b = &self.midi.zones.get(&master)?.binding;
        let instance = b.graph_instance?;
        let mut args = vec![
            OscType::Int(instance),
            OscType::Int(id),
            OscType::String(b.freq_control.clone()),
            OscType::Float(convert::midi2freq(f32::from(key) + bend)),
            OscType::String(b.amp_control.clone()),
            OscType::Float(convert::velocity2amp(velocity)),
        ];
        if let Some(name) = &b.pressure_control {
            args.push(OscType::String(name.clone()));
            args.push(OscType::Float(convert::aftertouch2control(pressure)));
        }
        if let Some(name) = &b.timbre_control {
            args.push(OscType::String(name.clone()));
            args.push(OscType::Float(convert::cc2control(timbre)));
        }
        Some(midi_message("/graph_newVoice", args))
    }

    /// What releases a zone's graph voice: its lift first when mapped, then
    /// the gate closing or the voice freed.
    fn zone_release(&self, voice: ZoneVoice, velocity: u16) -> Vec<rosc::OscMessage> {
        let Some(b) = self.midi.zones.get(&voice.master).map(|z| &z.binding) else {
            return vec![midi_message("/node_free", vec![OscType::Int(voice.id)])];
        };
        let mut out = Vec::new();
        if let Some(lift) = &b.lift_control {
            out.push(midi_message(
                "/node_set",
                vec![
                    OscType::Int(voice.id),
                    OscType::String(lift.clone()),
                    OscType::Float(convert::velocity2amp(velocity)),
                ],
            ));
        }
        out.push(release_message(b, voice.id));
        out
    }

    /// The controls a zone's expression sets on one of its graph voices.
    fn zone_expression(&self, voice: ZoneVoice, event: MpeEvent) -> Vec<(String, f32)> {
        let Some(b) = self.midi.zones.get(&voice.master).map(|z| &z.binding) else {
            return Vec::new();
        };
        match event {
            MpeEvent::Bend { semitones, .. } => {
                let mut out = vec![(
                    b.freq_control.clone(),
                    convert::midi2freq(f32::from(voice.key) + semitones),
                )];
                if let Some(name) = &b.bend_control {
                    out.push((name.clone(), semitones));
                }
                out
            }
            MpeEvent::Pressure { value, .. } => b
                .pressure_control
                .iter()
                .map(|n| (n.clone(), convert::aftertouch2control(value)))
                .collect(),
            MpeEvent::Timbre { value, .. } => b
                .timbre_control
                .iter()
                .map(|n| (n.clone(), convert::cc2control(value)))
                .collect(),
            MpeEvent::Controller {
                controller, value, ..
            } => {
                b.cc.get(&controller)
                    .map(|n| (n.clone(), convert::cc2control(value)))
                    .into_iter()
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    /// **A channel voice message** on its channel's route: an event on the
    /// bus, or a GraphDef binding's voice slots. On a plugin binding a
    /// controller, pressure or bend mapped to a control sets that control.
    /// Unbound channels are ignored (a running MIDI stream must never error).
    /// Network thread only.
    pub fn translate_midi(
        &mut self,
        msg: ChannelVoiceMessage,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let channel = message_channel(msg);
        match self.midi_route(channel) {
            Some(MidiRoute::Bus(bus)) => {
                #[cfg(all(
                    any(feature = "plugin-clap", feature = "plugin-vst3"),
                    target_os = "linux"
                ))]
                if let Some(node) = self.midi.channels.get(&channel).and_then(|b| b.node)
                    && matches!(self.node_defs.get(&node), Some(NodeDef::Plugin(..)))
                    && let Some((control, value)) = self.midi_expression(msg)
                    && self.node_defs[&node].control_index(&control).is_some()
                {
                    return self.translate(
                        &midi_message(
                            "/node_set",
                            vec![
                                OscType::Int(node),
                                OscType::String(control),
                                OscType::Float(value),
                            ],
                        ),
                        cmds,
                    );
                }
                cmds.push(Cmd::BusEvent {
                    bus,
                    event: to_bus_event(msg, midi_source(u16::from(channel))),
                });
                Ok(())
            }
            Some(MidiRoute::Graph) => self.graph_midi(msg, cmds),
            None => Ok(()),
        }
    }

    /// A channel voice message on a GraphDef binding.
    fn graph_midi(&mut self, msg: ChannelVoiceMessage, cmds: &mut Vec<Cmd>) -> Result<(), String> {
        use ChannelVoiceMessage::*;
        match msg {
            NoteOn {
                channel,
                note,
                velocity,
            } => {
                if velocity == 0 {
                    self.midi_note_off(channel, note, 0, cmds)
                } else {
                    self.midi_note_on(channel, note, velocity, cmds)
                }
            }
            NoteOff {
                channel,
                note,
                velocity,
            } => self.midi_note_off(channel, note, velocity, cmds),
            PolyAftertouch { channel, note, .. } => {
                if let Some((ctrl, value)) = self.midi_expression(msg)
                    && let Some(&id) = self.midi.voices.get(&(channel, note))
                {
                    self.midi_set(id, &ctrl, value, cmds);
                }
                Ok(())
            }
            ChannelAftertouch { channel, .. }
            | ControlChange { channel, .. }
            | PitchBend { channel, .. } => {
                if let Some((ctrl, value)) = self.midi_expression(msg) {
                    for id in self.midi.voice_ids(channel) {
                        self.midi_set(id, &ctrl, value, cmds);
                    }
                }
                Ok(())
            }
            ProgramChange { .. } => Ok(()),
        }
    }

    /// Note on, on a GraphDef binding: a voice slot. Retriggering a note
    /// already sounding releases the old voice first.
    fn midi_note_on(
        &mut self,
        channel: u8,
        note: u8,
        velocity: u16,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        if self.midi.voices.contains_key(&(channel, note)) {
            self.midi_note_off(channel, note, 0, cmds)?;
        }
        let id = self
            .midi
            .alloc_id()
            .ok_or("out of MIDI voice ids: ids recycle when their nodes end")?;
        let Some(msg) = self.midi_voice(channel, note, velocity, id) else {
            self.midi.release_id(id as i64);
            return Ok(());
        };
        if let Err(e) = self.translate(&msg, cmds) {
            self.midi.release_id(id as i64);
            return Err(e);
        }
        self.midi.voices.insert((channel, note), id);
        Ok(())
    }

    /// Note off, on a GraphDef binding: the slot released, with the release
    /// velocity on the `lift` control first when the binding maps one.
    fn midi_note_off(
        &mut self,
        channel: u8,
        note: u8,
        velocity: u16,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let Some(id) = self.midi.voices.remove(&(channel, note)) else {
            return Ok(());
        };
        if let Some(lift) = self
            .midi
            .channels
            .get(&channel)
            .and_then(|b| b.lift_control.clone())
        {
            self.midi_set(id, &lift, convert::velocity2amp(velocity), cmds);
        }
        if let Some(msg) = self.midi_release(channel, id) {
            // A freed voice may already be gone; an unknown control is a no-op.
            let _ = self.translate(&msg, cmds);
        }
        Ok(())
    }

    /// The `/graph_newVoice` that starts voice `id` for a note on `channel`,
    /// with `freq`/`amp` from the conversions. `None` when the channel is not
    /// bound to a GraphDef.
    fn midi_voice(
        &self,
        channel: u8,
        note: u8,
        velocity: u16,
        id: i32,
    ) -> Option<rosc::OscMessage> {
        let binding = self.midi.channels.get(&channel)?;
        let instance = binding.graph_instance?;
        Some(midi_message(
            "/graph_newVoice",
            vec![
                OscType::Int(instance),
                OscType::Int(id),
                OscType::String(binding.freq_control.clone()),
                OscType::Float(convert::midi2freq(note as f32)),
                OscType::String(binding.amp_control.clone()),
                OscType::Float(convert::velocity2amp(velocity)),
            ],
        ))
    }

    /// The message that releases graph voice `id` of `channel`.
    fn midi_release(&self, channel: u8, id: i32) -> Option<rosc::OscMessage> {
        Some(release_message(self.midi.channels.get(&channel)?, id))
    }

    /// **The control an expressive message moves**, and its value, as the
    /// channel's binding maps it: poly and channel pressure, a controller, a
    /// bend. `None` for any other message, an unbound channel or an unmapped
    /// one.
    fn midi_expression(&self, msg: ChannelVoiceMessage) -> Option<(String, f32)> {
        use ChannelVoiceMessage::*;
        let (channel, control, value) = match msg {
            PolyAftertouch {
                channel, pressure, ..
            } => (
                channel,
                Control::Poly,
                convert::aftertouch2control(pressure),
            ),
            ChannelAftertouch { channel, pressure } => (
                channel,
                Control::Pressure,
                convert::aftertouch2control(pressure),
            ),
            ControlChange {
                channel,
                controller,
                value,
            } => (channel, Control::Cc(controller), convert::cc2control(value)),
            PitchBend { channel, value } => (channel, Control::Bend, convert::bend2control(value)),
            _ => return None,
        };
        let binding = self.midi.channels.get(&channel)?;
        let name = match control {
            Control::Poly => binding.poly_control.clone(),
            Control::Pressure => binding.pressure_control.clone(),
            Control::Cc(n) => binding.cc.get(&n).cloned(),
            Control::Bend => binding.bend_control.clone(),
        }?;
        Some((name, value))
    }

    /// **A lane's MIDI note on a GraphDef binding**, built as a live note-on
    /// and its note-off would be, into `start` and `release`. Returns the
    /// voice id, or `None` when the channel is not bound to one.
    pub fn lane_midi_note(
        &mut self,
        channel: u8,
        note: u8,
        velocity: u16,
        start: &mut Vec<Cmd>,
        release: &mut Vec<Cmd>,
    ) -> Result<Option<i32>, String> {
        if self.midi_route(channel) != Some(MidiRoute::Graph) {
            return Ok(None);
        }
        let id = self
            .midi
            .alloc_id()
            .ok_or("out of MIDI voice ids: ids recycle when their nodes end")?;
        let Some(msg) = self.midi_voice(channel, note, velocity, id) else {
            self.midi.release_id(id as i64);
            return Ok(None);
        };
        if let Err(e) = self.translate(&msg, start) {
            self.midi.release_id(id as i64);
            return Err(e);
        }
        if let Some(msg) = self.midi_release(channel, id) {
            self.translate(&msg, release)?;
        }
        Ok(Some(id))
    }

    /// **A lane's note in a zone bound to a GraphDef**, into `start` and
    /// `release`. Returns the voice id, or `None` when there is no such zone.
    #[allow(clippy::too_many_arguments)] // one note's whole state, as the decoder hands it
    pub fn lane_zone_note(
        &mut self,
        master: u8,
        key: u8,
        velocity: u16,
        bend: f32,
        pressure: u32,
        timbre: u32,
        start: &mut Vec<Cmd>,
        release: &mut Vec<Cmd>,
    ) -> Result<Option<i32>, String> {
        if self.zone_route(master) != Some(MidiRoute::Graph) {
            return Ok(None);
        }
        let id = self
            .midi
            .alloc_id()
            .ok_or("out of MIDI voice ids: ids recycle when their nodes end")?;
        let Some(msg) = self.zone_voice(master, key, velocity, bend, pressure, timbre, id) else {
            self.midi.release_id(id as i64);
            return Ok(None);
        };
        if let Err(e) = self.translate(&msg, start) {
            self.midi.release_id(id as i64);
            return Err(e);
        }
        for msg in self.zone_release(ZoneVoice { id, key, master }, 0) {
            self.translate(&msg, release)?;
        }
        Ok(Some(id))
    }

    /// **A lane's per-note expression** on its graph voice `id`.
    pub fn lane_zone_set(
        &mut self,
        master: u8,
        key: u8,
        id: i32,
        event: MpeEvent,
        cmds: &mut Vec<Cmd>,
    ) {
        for (control, value) in self.zone_expression(ZoneVoice { id, key, master }, event) {
            self.midi_set(id, &control, value, cmds);
        }
    }

    /// **A lane's per-note message** on the graph voice `node`.
    pub fn lane_per_note(
        &mut self,
        node: i32,
        channel: u8,
        note: u8,
        message: crate::midi::ump::PerNote,
        cmds: &mut Vec<Cmd>,
    ) {
        self.per_note(node, channel, note, message, cmds);
    }

    /// **A lane's expressive MIDI message** on `voices`, the lane's graph
    /// voices it reaches.
    pub fn lane_midi_set(&mut self, msg: ChannelVoiceMessage, voices: &[i32], cmds: &mut Vec<Cmd>) {
        if let Some((control, value)) = self.midi_expression(msg) {
            for &id in voices {
                self.midi_set(id, &control, value, cmds);
            }
        }
    }

    /// **MIDI 2.0's packets** (`/midi_ump`, a lane's `ump`): each channel voice
    /// message as [`Self::translate_midi`] takes it, at the resolution it came
    /// in; a MIDI 1.0 message in a packet as the live input's bytes; and a
    /// per-note message as its note's expression -- or, on a GraphDef binding,
    /// on the voice sounding on its channel and key. Nothing a running stream
    /// sends errors.
    pub fn translate_ump(&mut self, words: &[u32], cmds: &mut Vec<Cmd>) -> Result<(), String> {
        use crate::midi::ump::{UmpMessage, parse_ump};
        let mut result = Ok(());
        for message in parse_ump(words) {
            let done = match message {
                UmpMessage::Voice(msg) => self.translate_midi(msg, cmds),
                UmpMessage::Midi1(bytes) => self.translate_midi_bytes(&bytes, cmds),
                UmpMessage::PerNote {
                    channel,
                    note,
                    message,
                } => {
                    match self.midi_route(channel) {
                        Some(MidiRoute::Bus(bus)) => {
                            if let Some(event) = per_note_event(
                                message,
                                midi_source(u16::from(channel)),
                                channel,
                                note,
                                -1,
                            ) {
                                cmds.push(Cmd::BusEvent { bus, event });
                            }
                        }
                        Some(MidiRoute::Graph) => {
                            if let Some(&id) = self.midi.voices.get(&(channel, note)) {
                                self.per_note(id, channel, note, message, cmds);
                            }
                        }
                        None => {}
                    }
                    Ok(())
                }
            };
            if done.is_err() && result.is_ok() {
                result = done;
            }
        }
        result
    }

    /// **A per-note message on graph voice `id`**, the note `note` of
    /// `channel`: a bend retunes it and sets the binding's bend control in
    /// semitones; a controller sets the control the binding maps that number
    /// to.
    pub(crate) fn per_note(
        &mut self,
        id: i32,
        channel: u8,
        note: u8,
        message: crate::midi::ump::PerNote,
        cmds: &mut Vec<Cmd>,
    ) {
        use crate::midi::ump::PerNote;
        let Some(binding) = self.midi.channels.get(&channel) else {
            return;
        };
        match message {
            PerNote::Bend(semitones) => {
                let bend = binding.bend_control.clone();
                let freq = binding.freq_control.clone();
                self.midi_set(
                    id,
                    &freq,
                    convert::midi2freq(f32::from(note) + semitones),
                    cmds,
                );
                if let Some(control) = bend {
                    self.midi_set(id, &control, semitones, cmds);
                }
            }
            PerNote::Controller { index, value } => {
                if let Some(control) = binding.cc.get(&index).cloned() {
                    self.midi_set(id, &control, convert::cc2control(value), cmds);
                }
            }
        }
    }

    /// `/midi_ump word...`: MIDI 2.0 packets, each word an int's 32 bits, played
    /// as the live input plays its messages ([`Self::translate_ump`]).
    pub(in crate::osc::translate) fn midi_ump(
        &mut self,
        msg: &rosc::OscMessage,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let words: Vec<u32> = msg
            .args
            .iter()
            .map(|arg| match arg {
                OscType::Int(word) => Ok(*word as u32),
                OscType::Long(word) => Ok(*word as u32),
                _ => Err("/midi_ump takes the packets' words as ints".to_string()),
            })
            .collect::<Result<_, _>>()?;
        self.translate_ump(&words, cmds)
    }

    /// `/node_set` one control on one voice; tolerate a stale node / unknown name.
    fn midi_set(&mut self, id: i32, control: &str, value: f32, cmds: &mut Vec<Cmd>) {
        let msg = midi_message(
            "/node_set",
            vec![
                OscType::Int(id),
                OscType::String(control.to_string()),
                OscType::Float(value),
            ],
        );
        let _ = self.translate(&msg, cmds);
    }
}

/// The channel a channel voice message is on.
pub(crate) fn message_channel(msg: ChannelVoiceMessage) -> u8 {
    use ChannelVoiceMessage::*;
    match msg {
        NoteOn { channel, .. }
        | NoteOff { channel, .. }
        | PolyAftertouch { channel, .. }
        | ChannelAftertouch { channel, .. }
        | ControlChange { channel, .. }
        | ProgramChange { channel, .. }
        | PitchBend { channel, .. } => channel,
    }
}

/// **A zone's per-note message as events**, from `source`: a note-on as the
/// note and the bend, pressure and timbre it starts with; a note-off; a bend
/// as the note's tuning, pressure and timbre as its expression; the sustain
/// pedal and another controller as the zone's.
pub(crate) fn zone_events(event: MpeEvent, source: u128, master: i16) -> Vec<BusEvent> {
    let note_of = |note: u32| i32::try_from(note & 0x7fff_ffff).unwrap_or(0);
    let expression = |note: u32, which: Expression, value: f64| BusEvent {
        note_id: note_of(note),
        index: which as u32,
        value,
        ..BusEvent::new(Kind::Expression, source)
    };
    match event {
        MpeEvent::NoteOn {
            note,
            channel,
            key,
            velocity,
            bend,
            pressure,
            timbre,
            ..
        } => vec![
            BusEvent {
                channel: i16::from(channel),
                key: i16::from(key),
                note_id: note_of(note),
                value: f64::from(velocity) / f64::from(u16::MAX),
                ..BusEvent::new(Kind::NoteOn, source)
            },
            expression(note, Expression::Tuning, f64::from(bend)),
            expression(
                note,
                Expression::Pressure,
                f64::from(pressure) / f64::from(u32::MAX),
            ),
            expression(
                note,
                Expression::Brightness,
                f64::from(timbre) / f64::from(u32::MAX),
            ),
        ],
        MpeEvent::NoteOff { note, velocity } => vec![BusEvent {
            note_id: note_of(note),
            value: f64::from(velocity) / f64::from(u16::MAX),
            ..BusEvent::new(Kind::NoteOff, source)
        }],
        MpeEvent::Bend { note, semitones } => {
            vec![expression(note, Expression::Tuning, f64::from(semitones))]
        }
        MpeEvent::Pressure { note, value } => vec![expression(
            note,
            Expression::Pressure,
            f64::from(value) / f64::from(u32::MAX),
        )],
        MpeEvent::Timbre { note, value } => vec![expression(
            note,
            Expression::Brightness,
            f64::from(value) / f64::from(u32::MAX),
        )],
        MpeEvent::Controller {
            controller, value, ..
        } => vec![BusEvent {
            channel: if controller == 64 { -1 } else { master },
            index: u32::from(controller),
            value: f64::from(value) / f64::from(u32::MAX),
            ..BusEvent::new(Kind::Controller, source)
        }],
        MpeEvent::Plain(_) => Vec::new(),
    }
}

/// **A MIDI 2.0 per-note message as its note's expression**: a bend as the
/// tuning, the timbre controller (74) as the brightness. The note is named
/// by its channel and key, or by `note_id` when the writer knows it. Other
/// per-note controllers have no event.
pub(crate) fn per_note_event(
    message: crate::midi::ump::PerNote,
    source: u128,
    channel: u8,
    key: u8,
    note_id: i32,
) -> Option<BusEvent> {
    use crate::midi::ump::PerNote;
    let (which, value) = match message {
        PerNote::Bend(semitones) => (Expression::Tuning, f64::from(semitones)),
        PerNote::Controller { index: 74, value } => (
            Expression::Brightness,
            f64::from(value) / f64::from(u32::MAX),
        ),
        PerNote::Controller { .. } => return None,
    };
    Some(BusEvent {
        channel: i16::from(channel),
        key: if note_id >= 0 { -1 } else { i16::from(key) },
        note_id,
        index: which as u32,
        value,
        ..BusEvent::new(Kind::Expression, source)
    })
}

/// What releases a graph voice: `gate 0` for a gate-aware binding, else
/// `/node_free`.
fn release_message(b: &MidiBinding, id: i32) -> rosc::OscMessage {
    if b.gate {
        midi_message(
            "/node_set",
            vec![
                OscType::Int(id),
                OscType::String(b.gate_control.clone()),
                OscType::Float(0.0),
            ],
        )
    } else {
        midi_message("/node_free", vec![OscType::Int(id)])
    }
}

/// Which of a binding's controls an expressive message moves.
enum Control {
    Poly,
    Pressure,
    Cc(u8),
    Bend,
}

/// A MIDI channel argument: 0-based, the classic 16 plus the extended UMP
/// group*channel space (0..=255).
fn midi_channel(channel: i32) -> Result<u8, String> {
    u8::try_from(channel).map_err(|_| "MIDI channel out of range (0-255)".to_string())
}

/// Builds the OSC message a MIDI event becomes, fed back through
/// [`CmdTranslator::translate`] for byte-identical parity with the OSC path.
fn midi_message(addr: &str, args: Vec<OscType>) -> rosc::OscMessage {
    rosc::OscMessage {
        addr: addr.to_string(),
        args,
    }
}
