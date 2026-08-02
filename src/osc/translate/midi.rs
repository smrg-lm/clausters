//! MIDI: the bindings a channel carries, and the nodes they actuate.
//!
//! A binding says what a channel plays — an instrument def, where in the tree,
//! whether notes gate — and `/midi_map` points a controller at a control. A
//! channel-voice message is turned into the OSC message the same event would
//! have arrived as and fed back through [`CmdTranslator::translate`], so the
//! MIDI path and the OSC path build byte-identical commands.

use super::*;

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
    /// `None` (a plain def is `/synth_new`'d per note). A GraphDef with no
    /// per-voice members is rejected — it has nothing to play per note. Shared
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

    /// re-establish a persisted binding at startup, re-instantiating its
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
    pub(in crate::osc::translate) fn midi_unbind(
        &mut self,
        msg: &rosc::OscMessage,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
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
    pub(in crate::osc::translate) fn midi_map(
        &mut self,
        msg: &rosc::OscMessage,
    ) -> Result<(), String> {
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

    /// actuate nodes from a standard channel-voice MIDI message. Reuses
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

    /// Note on → `/synth_new` with `freq`/`amp` from the conversions. Retriggering
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
        let id = self
            .midi
            .alloc_id()
            .ok_or("out of MIDI voice ids: ids recycle when their nodes end")?;
        let freq = OscType::Float(convert::midi2freq(note as f32));
        let amp = OscType::Float(convert::velocity2amp(velocity));
        // A GraphDef binding spawns a per-voice sub-graph into the shared
        // instance; a plain def spawns a synth. Both carry freq/amp as the
        // surface/control values.
        let msg = match graph_instance {
            Some(instance) => midi_message(
                "/graph_newVoice",
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
                "/synth_new",
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
        if let Err(e) = self.translate(&msg, cmds) {
            self.midi.release_id(id as i64);
            return Err(e);
        }
        self.midi.voices.insert((channel, note), id);
        Ok(())
    }

    /// Note off → `/node_free` (or `/node_set gate 0` for gate-aware bindings).
    fn midi_note_off(&mut self, channel: u8, note: u8, cmds: &mut Vec<Cmd>) -> Result<(), String> {
        let Some(id) = self.midi.voices.remove(&(channel, note)) else {
            return Ok(());
        };
        let gate = self.midi.channels.get(&channel);
        let msg = match gate.filter(|b| b.gate) {
            Some(b) => midi_message(
                "/node_set",
                vec![
                    OscType::Int(id),
                    OscType::String(b.gate_control.clone()),
                    OscType::Float(0.0),
                ],
            ),
            None => midi_message("/node_free", vec![OscType::Int(id)]),
        };
        // A freed voice may already be gone; an unknown control is a no-op.
        let _ = self.translate(&msg, cmds);
        Ok(())
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
