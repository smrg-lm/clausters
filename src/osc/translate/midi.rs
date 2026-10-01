//! MIDI: the bindings a channel carries, and the nodes they actuate.
//!
//! A binding says what a channel plays -- an instrument def, where in the tree,
//! whether notes gate -- and `/midi_map` points a controller at a control. A
//! channel-voice message is turned into the OSC message the same event would
//! have arrived as and fed back through [`CmdTranslator::translate`], so the
//! MIDI path and the OSC path build byte-identical commands.

use clausters_midi::mpe::{MpeEvent, Side};

use super::*;
use crate::midi::{ZoneBinding, ZoneVoice};
use crate::osc::args::Args;

impl CmdTranslator {
    /// `/midi_bind channel instrument [target] [addAction] [gate]`: bind a MIDI
    /// channel to an instrument def (SynthDef *or* FaustDef *or* GraphDef).
    /// Default control map is `freq`/`amp`; `/midi_map` extends it. When the
    /// instrument is a **GraphDef** (with per-voice members), the shared
    /// instance is spawned now and each note becomes a `/graph_newVoice`.
    pub(in crate::osc::translate) fn midi_bind(
        &mut self,
        msg: &rosc::OscMessage,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let [OscType::Int(channel), OscType::String(instrument), ..] = msg.args.as_slice() else {
            return Err("expected: channel, instrument [, target, addAction, gate]".into());
        };
        let channel = midi_channel(*channel)?;
        let mut tail = Args::after(&msg.args, 2);
        let target = tail.opt_int()?.unwrap_or(0);
        let action = tail.opt_int()?.unwrap_or(0);
        let gate = tail.opt_int()?.unwrap_or(0) != 0;
        if AddAction::from_i32(action).is_none() {
            return Err("add action must be 0-4".into());
        }
        if channel < 16 && self.midi.decoder.zone_of(channel).is_some() {
            return Err(format!(
                "channel {channel} is in an MPE zone: unbind the zone first"
            ));
        }
        let mut binding = MidiBinding::new(instrument.clone(), target, action, gate);
        binding.graph_instance = self.bind_graph_instance(instrument, target, action, cmds)?;
        self.midi.channels.insert(channel, binding);
        self.midi.refresh_allowed();
        Ok(())
    }

    /// `/midi_bindZone master members instrument [target] [addAction] [gate]`:
    /// bind an **MPE zone** to an instrument. `master` is channel 0 (the lower
    /// zone, members ascending from 1) or 15 (the upper, descending from 14);
    /// `members` is how many (0 waits for the device's RPN 6, which wins
    /// whenever it comes). Each note is a voice of its own, and a member's
    /// bend, pressure and timbre reach that voice only -- bend as pitch, the
    /// member and master bends summed through their ranges. A zone whose
    /// channels reach one a per-channel binding holds is refused.
    pub(in crate::osc::translate) fn midi_bind_zone(
        &mut self,
        msg: &rosc::OscMessage,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let [
            OscType::Int(master),
            OscType::Int(members),
            OscType::String(instrument),
            ..,
        ] = msg.args.as_slice()
        else {
            return Err("expected: master, members, instrument [, target, addAction, gate]".into());
        };
        let side = u8::try_from(*master)
            .ok()
            .and_then(Side::of_master)
            .ok_or("a zone's master is channel 0 (lower) or 15 (upper)")?;
        let members = u8::try_from(*members)
            .ok()
            .filter(|m| *m <= 15)
            .ok_or("a zone has 0-15 members")?;
        let mut tail = Args::after(&msg.args, 3);
        let target = tail.opt_int()?.unwrap_or(0);
        let action = tail.opt_int()?.unwrap_or(0);
        let gate = tail.opt_int()?.unwrap_or(0) != 0;
        if AddAction::from_i32(action).is_none() {
            return Err("add action must be 0-4".into());
        }
        let mut zone = ZoneBinding::new(members, instrument.clone(), target, action, gate);
        self.bind_zone(side, &mut zone, cmds)?;
        self.midi.zones.insert(side.master(), zone);
        Ok(())
    }

    /// Puts `zone` on `side`: refused over a bound channel, its shared
    /// GraphDef instance spawned, the decoder told. A zone already there is
    /// unbound first. Shared by `/midi_bindZone` and the restore.
    fn bind_zone(
        &mut self,
        side: Side,
        zone: &mut ZoneBinding,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
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
        let b = &zone.binding;
        zone.binding.graph_instance =
            self.bind_graph_instance(&b.instrument.clone(), b.target, b.action, cmds)?;
        self.midi.decoder.set_zone(side, Some(zone.config.members));
        self.midi.decoder.set_timbre_cc(side, zone.config.timbre_cc);
        // What the zone's notes end on is freed with them below, not replayed.
        while self.midi.decoder.poll().is_some() {}
        self.midi.refresh_allowed();
        Ok(())
    }

    /// Unbinds the zone on `master`: its voices (or its shared instance) are
    /// freed and the decoder forgets it.
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
        let ids: Vec<i32> = notes
            .iter()
            .filter_map(|n| self.midi.zone_voices.remove(n).map(|v| v.id))
            .collect();
        if let Some(instance) = zone.binding.graph_instance {
            cmds.push(Cmd::FreeNode { id: instance });
            self.mirror.remove(instance);
            self.free_graph_node(instance);
        } else {
            for id in ids {
                cmds.push(Cmd::FreeNode { id });
                self.mirror.remove(id);
            }
        }
    }

    /// If `instrument` names a GraphDef, spawn its shared instance now (so each
    /// note spawns a voice into it) and return the instance id; otherwise
    /// `None` (a plain def is `/synth_new`'d per note). A GraphDef with no
    /// per-voice members is rejected -- it has nothing to play per note. Shared
    /// by `/midi_bind` and the binding restore.
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

    /// Re-establish a persisted binding at startup, re-instantiating its
    /// shared GraphDef instance if needed. Mirrors `/midi_bind` but takes the
    /// stored config directly (no re-issued OSC).
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
        binding.graph_instance =
            self.bind_graph_instance(&binding.instrument, binding.target, binding.action, cmds)?;
        self.midi.channels.insert(pb.channel, binding);
        self.midi.refresh_allowed();
        Ok(())
    }

    /// `/midi_unbind channel`: drop the binding and free every voice still
    /// sounding on that channel (and, for a GraphDef binding, its shared
    /// instance -- which frees the voices with it).
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
        let instance = self
            .midi
            .channels
            .remove(&channel)
            .and_then(|b| b.graph_instance);
        self.midi.refresh_allowed();
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

    /// `/midi_map channel selector name [cc]`: route a message type to a
    /// control. Selectors: `note`, `vel`, `gate`, `bend`, `pressure` (channel
    /// aftertouch), `poly` (per-note aftertouch), `lift` (note-off velocity),
    /// `ccN` (control change), `progN` (program -> instrument def `name`), and
    /// on a zone's master `timbre` (its third dimension; a fourth argument sets
    /// the controller that carries it, 74 unless given).
    pub(in crate::osc::translate) fn midi_map(
        &mut self,
        msg: &rosc::OscMessage,
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
        if let Some(zone) = self.midi.zones.get_mut(&channel) {
            if selector == "timbre" {
                zone.binding.timbre_control = Some(name.clone());
                if let Some(OscType::Int(cc)) = tail.first() {
                    let cc = u8::try_from(*cc)
                        .ok()
                        .filter(|c| *c < 128)
                        .ok_or("a controller is 0-127")?;
                    zone.config.timbre_cc = cc;
                    if let Some(side) = Side::of_master(channel) {
                        self.midi.decoder.set_timbre_cc(side, cc);
                    }
                }
                return Ok(());
            }
        } else if selector == "timbre" {
            return Err("timbre is a zone's third dimension: bind one with /midi_bindZone".into());
        }
        let binding = match self.midi.zones.get_mut(&channel) {
            Some(zone) => &mut zone.binding,
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
            "lift" => binding.lift_control = Some(name.clone()),
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

    /// **A raw MIDI message**, as the live input delivers it: through the
    /// zones' decoder, whose per-note messages actuate a zone's voices and
    /// whose untouched ones are a plain channel's ([`Self::translate_midi`]).
    /// Unbound channels and anything else that plays nothing are ignored.
    pub fn translate_midi_bytes(
        &mut self,
        bytes: &[u8],
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        self.midi.decoder.feed(bytes);
        let mut result = Ok(());
        while let Some(event) = self.midi.decoder.poll() {
            let done = match event {
                MpeEvent::Plain([status, d1, d2]) => match crate::midi::parse_midi1(status, d1, d2)
                {
                    Some(msg) => self.translate_midi(msg, cmds),
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

    /// **One per-note message of a zone**: a note-on makes a voice of the
    /// zone's instrument at its key plus its bend, with its pressure and
    /// timbre; a bend retunes that voice (and sets the `bend` control, in
    /// semitones, when one is mapped); pressure, timbre and a mapped
    /// controller set theirs; a note-off releases it, its lift first.
    fn translate_mpe(&mut self, event: MpeEvent, cmds: &mut Vec<Cmd>) -> Result<(), String> {
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
                if !self.midi.zones.contains_key(&master) {
                    return Ok(());
                }
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

    /// The message that starts voice `id` of the zone on `master`: its
    /// binding's instrument (or a voice of its shared GraphDef), `freq` at
    /// `key + bend` semitones, `amp` from the velocity, and the pressure and
    /// timbre controls the note starts with. `None` when no zone is there.
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
        let mut controls = vec![
            OscType::String(b.freq_control.clone()),
            OscType::Float(convert::midi2freq(f32::from(key) + bend)),
            OscType::String(b.amp_control.clone()),
            OscType::Float(convert::velocity2amp(velocity)),
        ];
        if let Some(name) = &b.pressure_control {
            controls.push(OscType::String(name.clone()));
            controls.push(OscType::Float(convert::aftertouch2control(pressure)));
        }
        if let Some(name) = &b.timbre_control {
            controls.push(OscType::String(name.clone()));
            controls.push(OscType::Float(convert::cc2control(timbre)));
        }
        let mut args = match b.graph_instance {
            Some(instance) => vec![OscType::Int(instance), OscType::Int(id)],
            None => vec![
                OscType::String(b.instrument.clone()),
                OscType::Int(id),
                OscType::Int(b.action),
                OscType::Int(b.target),
            ],
        };
        args.extend(controls);
        let addr = if b.graph_instance.is_some() {
            "/graph_newVoice"
        } else {
            "/synth_new"
        };
        Some(midi_message(addr, args))
    }

    /// What releases a zone's voice: its lift first when mapped, then the gate
    /// closing or the voice freed.
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
        out.push(if b.gate {
            midi_message(
                "/node_set",
                vec![
                    OscType::Int(voice.id),
                    OscType::String(b.gate_control.clone()),
                    OscType::Float(0.0),
                ],
            )
        } else {
            midi_message("/node_free", vec![OscType::Int(voice.id)])
        });
        out
    }

    /// The controls a zone's expression sets on one of its voices: a bend's
    /// pitch (and its semitones on a mapped `bend`), pressure, timbre, or a
    /// controller the binding maps.
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

    /// Actuate nodes from a standard channel-voice MIDI message. Reuses
    /// the OSC path by synthesizing the equivalent `/synth_new`/`/node_set`/`/node_free`,
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
                    self.midi_set_channel(channel, &ctrl, value, cmds);
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

    /// Note on -> `/synth_new` with `freq`/`amp` from the conversions. Retriggering
    /// a note already sounding frees the old voice first.
    fn midi_note_on(
        &mut self,
        channel: u8,
        note: u8,
        velocity: u16,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        if !self.midi.channels.contains_key(&channel) {
            return Ok(());
        }
        if self.midi.voices.contains_key(&(channel, note)) {
            self.midi_note_off(channel, note, 0, cmds)?;
        }
        let id = self
            .midi
            .alloc_id()
            .ok_or("out of MIDI voice ids: ids recycle when their nodes end")?;
        let Some(msg) = self.midi_voice(channel, note, velocity, id, None) else {
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

    /// Note off -> `/node_free` (or `/node_set gate 0` for gate-aware
    /// bindings), with the release velocity on the `lift` control first when
    /// the binding maps one.
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
        let msg = self.midi_release(channel, id);
        // A freed voice may already be gone; an unknown control is a no-op.
        let _ = self.translate(&msg, cmds);
        Ok(())
    }

    /// **The message that starts voice `id`** for a note on `channel`, as its
    /// binding says: a `/graph_newVoice` into a GraphDef binding's shared
    /// instance, else a `/synth_new` of the instrument -- `program`'s, when
    /// one is given and the binding maps it, else the binding's own -- with
    /// `freq`/`amp` from the conversions. `None` when the channel is unbound.
    fn midi_voice(
        &self,
        channel: u8,
        note: u8,
        velocity: u16,
        id: i32,
        program: Option<u8>,
    ) -> Option<rosc::OscMessage> {
        let binding = self.midi.channels.get(&channel)?;
        let freq = OscType::Float(convert::midi2freq(note as f32));
        let amp = OscType::Float(convert::velocity2amp(velocity));
        let freq_control = OscType::String(binding.freq_control.clone());
        let amp_control = OscType::String(binding.amp_control.clone());
        // A GraphDef binding spawns a per-voice sub-graph into the shared
        // instance; a plain def spawns a synth. Both carry freq/amp as the
        // surface/control values.
        Some(match binding.graph_instance {
            Some(instance) => midi_message(
                "/graph_newVoice",
                vec![
                    OscType::Int(instance),
                    OscType::Int(id),
                    freq_control,
                    freq,
                    amp_control,
                    amp,
                ],
            ),
            None => {
                let instrument = program
                    .and_then(|p| binding.programs.get(&p))
                    .unwrap_or(&binding.instrument);
                midi_message(
                    "/synth_new",
                    vec![
                        OscType::String(instrument.clone()),
                        OscType::Int(id),
                        OscType::Int(binding.action),
                        OscType::Int(binding.target),
                        freq_control,
                        freq,
                        amp_control,
                        amp,
                    ],
                )
            }
        })
    }

    /// **The message that releases voice `id`** of `channel`: `/node_set
    /// gate 0` for a gate-aware binding, else `/node_free`.
    fn midi_release(&self, channel: u8, id: i32) -> rosc::OscMessage {
        match self.midi.channels.get(&channel).filter(|b| b.gate) {
            Some(b) => midi_message(
                "/node_set",
                vec![
                    OscType::Int(id),
                    OscType::String(b.gate_control.clone()),
                    OscType::Float(0.0),
                ],
            ),
            None => midi_message("/node_free", vec![OscType::Int(id)]),
        }
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

    /// **A lane's MIDI note**, built as a live note-on and its note-off would
    /// be through the channel's binding, into `start` and `release`: its voice
    /// id is the MIDI range's, and `program` picks the instrument as a program
    /// change on the lane before it did. Returns the voice id, or `None` when
    /// the channel is unbound -- a lane's note on it sounds nothing, as a live
    /// one does. The voice is not one of the channel's live voices: a live
    /// message does not reach it, nor a lane's message a live one.
    pub fn lane_midi_note(
        &mut self,
        channel: u8,
        note: u8,
        velocity: u16,
        program: Option<u8>,
        start: &mut Vec<Cmd>,
        release: &mut Vec<Cmd>,
    ) -> Result<Option<i32>, String> {
        if !self.midi.channels.contains_key(&channel) {
            return Ok(None);
        }
        let id = self
            .midi
            .alloc_id()
            .ok_or("out of MIDI voice ids: ids recycle when their nodes end")?;
        let Some(msg) = self.midi_voice(channel, note, velocity, id, program) else {
            self.midi.release_id(id as i64);
            return Ok(None);
        };
        if let Err(e) = self.translate(&msg, start) {
            self.midi.release_id(id as i64);
            return Err(e);
        }
        let msg = self.midi_release(channel, id);
        self.translate(&msg, release)?;
        Ok(Some(id))
    }

    /// **A lane's note in an MPE zone**, built as the live note-on and its
    /// note-off would be through the zone on `master`, into `start` and
    /// `release`. Returns the voice id, or `None` when no zone is bound there.
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
        if !self.midi.zones.contains_key(&master) {
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

    /// **A lane's per-note expression** on its note's voice `id`, as the zone
    /// on `master` maps it.
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

    /// **A lane's expressive MIDI message** on `voices`, the lane's voices it
    /// reaches: the control the binding maps it to, set on each. Nothing when
    /// the binding maps none.
    /// **A lane's per-note message** on the voice `node`, the note `note` of
    /// `channel`, into `cmds` (`Self::per_note`).
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

    pub fn lane_midi_set(&mut self, msg: ChannelVoiceMessage, voices: &[i32], cmds: &mut Vec<Cmd>) {
        if let Some((control, value)) = self.midi_expression(msg) {
            for &id in voices {
                self.midi_set(id, &control, value, cmds);
            }
        }
    }

    /// **MIDI 2.0's packets** (`/midi_ump`, a lane's `ump`): each channel voice
    /// message plays as [`Self::translate_midi`] plays it, at the resolution
    /// it came in; a MIDI 1.0 message in a packet as the live input's bytes
    /// do; and a per-note message reaches the voice sounding on its channel
    /// and key (`Self::per_note`). Nothing a running stream sends errors.
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
                    if let Some(&id) = self.midi.voices.get(&(channel, note)) {
                        self.per_note(id, channel, note, message, cmds);
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

    /// **A per-note message on voice `id`**, the note `note` of `channel`: a
    /// bend retunes it (`freq` at its key plus the bend) and sets the
    /// binding's bend control, when one is mapped, in semitones -- as a zone's
    /// voice is bent; a controller sets the control the binding maps that
    /// number to, since a registered per-note controller's number is a CC's
    /// meaning (74, the timbre) and an assignable one is the CC its number
    /// names.
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
                self.midi_set(
                    id,
                    "freq",
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

    /// `/node_set` one control on every live voice of a channel.
    fn midi_set_channel(&mut self, channel: u8, control: &str, value: f32, cmds: &mut Vec<Cmd>) {
        for id in self.midi.voice_ids(channel) {
            self.midi_set(id, control, value, cmds);
        }
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
