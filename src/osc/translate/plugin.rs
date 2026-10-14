//! Native events resolve stable ports off RT, then target one persistent node.
use super::*;
use crate::midi::ZoneVoice;
use clausters_core::plugin_shm::{Event, EventKind};
use serde::Deserialize;

/// Client origins are distinct from reserved live-MIDI and lane origins.
pub(super) type MidiAddress = (i32, u64, u128, i16, i16, i16);

pub const MIDI_SOURCE: u128 = 1 << 95;
pub fn lane_source(lane: i32, generation: u32, transport: usize) -> u128 {
    (1_u128 << 96)
        | ((transport as u128) << 64)
        | ((lane as u32 as u128) << 32)
        | generation as u128
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    kind: String,
    #[serde(default)]
    source: u32,
    #[serde(default = "wildcard")]
    note_id: i32,
    port: Option<u32>,
    #[serde(default)]
    channel: i16,
    #[serde(default = "wildcard_key")]
    key: i16,
    #[serde(default)]
    id: u32,
    #[serde(default)]
    value: f64,
    bytes: Option<[u8; 3]>,
}
fn wildcard() -> i32 {
    -1
}
fn wildcard_key() -> i16 {
    -1
}
impl CmdTranslator {
    pub(super) fn plugin_event(
        &mut self,
        msg: &rosc::OscMessage,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        self.plugin_event_scoped(msg, None, cmds)
    }
    pub(crate) fn plugin_event_scoped(
        &mut self,
        msg: &rosc::OscMessage,
        source: Option<u128>,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let Some(OscType::Int(node)) = msg.args.first() else {
            return Err("expected plugin node and event JSON".into());
        };
        let p: Payload = serde_json::from_slice(crate::osc::args::json_payload(&msg.args[1..])?)
            .map_err(|e| format!("invalid plugin event: {e}"))?;
        let kind = match p.kind.as_str() {
            "note_on" => EventKind::NoteOn,
            "note_off" => EventKind::NoteOff,
            "choke" => EventKind::NoteChoke,
            "expression" => EventKind::Expression,
            "midi" => EventKind::Midi,
            _ => return Err("unsupported plugin event kind".into()),
        };
        let id = if kind == EventKind::Midi {
            let bytes = p.bytes.ok_or("MIDI event needs three bytes")?;
            if crate::midi::parse_midi1(bytes[0], bytes[1], bytes[2]).is_none()
                || bytes[1] > 127
                || bytes[2] > 127
            {
                return Err("invalid plugin MIDI message".into());
            }
            u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16)
        } else {
            p.id
        };
        self.plugin_event_cmd(
            *node,
            source.unwrap_or(p.source as u128),
            p.port,
            Event {
                kind: kind as u32,
                id,
                note_id: p.note_id,
                channel: p.channel,
                key: p.key,
                value: p.value,
                ..Event::default()
            },
            cmds,
        )
    }
    /// Validate dialect, native port and values before entering the audio FIFO.
    pub(crate) fn plugin_event_cmd(
        &mut self,
        node: i32,
        source: u128,
        port: Option<u32>,
        mut event: Event,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let Some(NodeDef::Plugin(def, generation, ..)) = self.node_defs.get(&node) else {
            return Err("not a plugin node".into());
        };
        let index = match port {
            Some(id) => def.info.note_inputs.iter().position(|p| p.id == id),
            None => (!def.info.note_inputs.is_empty()).then_some(0),
        }
        .ok_or("unknown plugin note port")?;
        let dialects = def.info.note_inputs[index].dialects;
        if event.kind == EventKind::Expression as u32 && dialects & 1 == 0 {
            return Err("plugin port does not support native per-note expression".into());
        }
        if event.kind == EventKind::Expression as u32 && event.id > 6 {
            return Err("unsupported plugin note expression".into());
        }
        if event.kind == EventKind::Midi as u32 && dialects & 6 == 0 {
            // Notes, sustain, pressure, bend and standard expression controllers
            // can be translated to native note events; others have no dialect.
            let status = event.id as u8 & 0xf0;
            let cc = (event.id >> 8) as u8;
            if !matches!(status, 0x80 | 0x90 | 0xa0 | 0xd0 | 0xe0)
                && !(status == 0xb0 && matches!(cc, 7 | 10 | 11 | 64 | 74 | 120 | 123))
            {
                return Err("plugin port does not support this MIDI controller/dialect".into());
            }
        }
        if dialects & 7 == 0 {
            return Err("unsupported plugin event dialect".into());
        }
        if !event.value.is_finite()
            || event.note_id < -1
            || !(-1..=15).contains(&event.channel)
            || !(-1..=127).contains(&event.key)
        {
            return Err("invalid plugin note address/value".into());
        }
        if (matches!(event.kind, 1 | 2) && !(0.0..=1.0).contains(&event.value))
            || (event.kind == 1 && (event.key < 0 || event.channel < 0))
        {
            return Err("invalid plugin note key/channel/velocity".into());
        }
        event.port = index as i16;
        let generation = *generation;
        // MIDI's key-only releases need an identity allocated at translation,
        // including notes built into future bundles. Internal negative ids
        // are disjoint from the explicit nonnegative client note ids.
        if event.kind == 5 && matches!(event.id as u8 & 0xf0, 0x80 | 0x90) {
            let bytes = [
                event.id as u8,
                (event.id >> 8) as u8,
                (event.id >> 16) as u8,
            ];
            event.kind = if bytes[0] & 0xf0 == 0x90 && bytes[2] != 0 {
                1
            } else {
                2
            };
            event.note_id = -1;
            event.channel = (bytes[0] & 15) as i16;
            event.key = bytes[1] as i16;
            event.value = bytes[2] as f64 / 127.0;
        }
        if matches!(event.kind, 1 | 2) && event.note_id == -1 {
            let key = (
                node,
                generation,
                source,
                event.port,
                event.channel,
                event.key,
            );
            if event.kind == 1 {
                let count = self.plugin_note_counts.entry(node).or_default();
                if *count == crate::plugin::HELD_NOTES {
                    return Err("plugin MIDI ownership capacity exceeded".into());
                }
                let token = self.next_plugin_note;
                self.next_plugin_note = token
                    .checked_sub(1)
                    .ok_or("plugin note identity capacity exceeded")?;
                self.plugin_note_queues
                    .entry(key)
                    .or_default()
                    .push_back(token);
                *count += 1;
                event.note_id = token;
            } else {
                let Some(queue) = self.plugin_note_queues.get_mut(&key) else {
                    return Ok(());
                };
                let Some(token) = queue.pop_front() else {
                    return Ok(());
                };
                if queue.is_empty() {
                    self.plugin_note_queues.remove(&key);
                }
                *self.plugin_note_counts.entry(node).or_default() -= 1;
                event.note_id = token;
            }
        }
        cmds.push(Cmd::PluginEvent {
            id: node,
            generation,
            source,
            event,
        });
        Ok(())
    }
    /// Shared decoded channel voice, converted without spawning a note node.
    pub(crate) fn plugin_midi(
        &mut self,
        node: i32,
        source: u128,
        msg: ChannelVoiceMessage,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        use ChannelVoiceMessage::*;
        let mut event = Event {
            note_id: -1,
            key: -1,
            kind: EventKind::Midi as u32,
            ..Event::default()
        };
        if matches!(self.node_defs.get(&node), Some(NodeDef::Plugin(def, ..)) if def.info.note_inputs.first().is_some_and(|p| p.dialects & 1 != 0))
        {
            let expression = match msg {
                PolyAftertouch {
                    channel,
                    note,
                    pressure,
                } => Some((channel, note as i16, 6, pressure as f64 / u32::MAX as f64)),
                ChannelAftertouch { channel, pressure } => {
                    Some((channel, -1, 6, pressure as f64 / u32::MAX as f64))
                }
                PitchBend { channel, value } => Some((
                    channel,
                    -1,
                    2,
                    (value as f64 / u32::MAX as f64 * 2.0 - 1.0) * 2.0,
                )),
                ControlChange {
                    channel,
                    controller,
                    value,
                } => match controller {
                    7 => Some((channel, -1, 0, value as f64 / u32::MAX as f64)),
                    10 => Some((channel, -1, 1, value as f64 / u32::MAX as f64 * 2.0 - 1.0)),
                    11 => Some((channel, -1, 4, value as f64 / u32::MAX as f64)),
                    74 => Some((channel, -1, 5, value as f64 / u32::MAX as f64)),
                    _ => None,
                },
                _ => None,
            };
            if let Some((channel, key, id, value)) = expression {
                return self.plugin_event_cmd(
                    node,
                    source,
                    None,
                    Event {
                        kind: 6,
                        note_id: -1,
                        channel: channel as i16,
                        key,
                        id,
                        value,
                        ..Event::default()
                    },
                    cmds,
                );
            }
        }
        let bytes = match msg {
            NoteOn {
                channel,
                note,
                velocity,
            } => {
                event.kind = if velocity == 0 {
                    EventKind::NoteOff
                } else {
                    EventKind::NoteOn
                } as u32;
                event.channel = channel as i16;
                event.key = note as i16;
                event.value = velocity as f64 / 65535.0;
                return self.plugin_event_cmd(node, source, None, event, cmds);
            }
            NoteOff {
                channel,
                note,
                velocity,
            } => {
                event.kind = EventKind::NoteOff as u32;
                event.channel = channel as i16;
                event.key = note as i16;
                event.value = velocity as f64 / 65535.0;
                return self.plugin_event_cmd(node, source, None, event, cmds);
            }
            PolyAftertouch {
                channel,
                note,
                pressure,
            } => [0xa0 | channel, note, (pressure >> 25) as u8],
            ChannelAftertouch { channel, pressure } => [0xd0 | channel, (pressure >> 25) as u8, 0],
            ControlChange {
                channel,
                controller,
                value,
            } => [0xb0 | channel, controller, (value >> 25) as u8],
            ProgramChange { channel, program } => [0xc0 | channel, program, 0],
            PitchBend { channel, value } => {
                let v = value >> 18;
                [0xe0 | channel, (v & 127) as u8, (v >> 7) as u8]
            }
        };
        event.id = bytes[0] as u32 | ((bytes[1] as u32) << 8) | ((bytes[2] as u32) << 16);
        self.plugin_event_cmd(node, source, None, event, cmds)
    }
}

impl CmdTranslator {
    pub(crate) fn plugin_per_note(
        &mut self,
        node: i32,
        source: u128,
        channel: u8,
        key: u8,
        message: crate::midi::ump::PerNote,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let (id, value) = match message {
            crate::midi::ump::PerNote::Bend(value) => (2, value as f64),
            crate::midi::ump::PerNote::Controller { index: 74, value } => {
                (5, value as f64 / u32::MAX as f64)
            }
            _ => return Err("unsupported native per-note controller/dialect".into()),
        };
        self.plugin_event_cmd(
            node,
            source,
            None,
            Event {
                kind: EventKind::Expression as u32,
                note_id: -1,
                channel: channel as i16,
                key: key as i16,
                id,
                value,
                ..Event::default()
            },
            cmds,
        )
    }
    pub(super) fn plugin_mpe(
        &mut self,
        event: clausters_midi::mpe::MpeEvent,
        cmds: &mut Vec<Cmd>,
    ) -> Result<bool, String> {
        use clausters_midi::mpe::MpeEvent::*;
        if let NoteOn {
            note,
            side,
            channel,
            key,
            velocity,
            bend,
            pressure,
            timbre,
        } = event
        {
            let master = side.master();
            let Some(node) = self
                .midi
                .zones
                .get(&master)
                .and_then(|z| z.binding.graph_instance)
            else {
                return Ok(false);
            };
            if !matches!(self.node_defs.get(&node), Some(NodeDef::Plugin(..))) {
                return Ok(false);
            }
            if !matches!(self.node_defs.get(&node), Some(NodeDef::Plugin(def, ..)) if def.info.note_inputs.first().is_some_and(|p| p.dialects & 1 != 0))
            {
                return Err("native MPE requires a CLAP note-expression port".into());
            }
            let note_id =
                i32::try_from(note).map_err(|_| "native note identity capacity exceeded")?;
            let source = MIDI_SOURCE | 256 | master as u128;
            let address = Event {
                note_id,
                channel: channel as i16,
                key: key as i16,
                ..Event::default()
            };
            self.plugin_event_cmd(
                node,
                source,
                None,
                Event {
                    kind: EventKind::NoteOn as u32,
                    value: velocity as f64 / 65535.0,
                    ..address
                },
                cmds,
            )?;
            for (id, value) in [
                (2, bend as f64),
                (6, pressure as f64 / u32::MAX as f64),
                (5, timbre as f64 / u32::MAX as f64),
            ] {
                self.plugin_event_cmd(
                    node,
                    source,
                    None,
                    Event {
                        kind: EventKind::Expression as u32,
                        id,
                        value,
                        ..address
                    },
                    cmds,
                )?;
            }
            self.midi.zone_voices.insert(
                note,
                ZoneVoice {
                    id: node,
                    key,
                    master,
                },
            );
            return Ok(true);
        }
        let (note, kind, id, value) =
            match event {
                NoteOff { note, velocity } => {
                    (note, EventKind::NoteOff, 0, velocity as f64 / 65535.0)
                }
                Bend { note, semitones } => (note, EventKind::Expression, 2, semitones as f64),
                Pressure { note, value } => (
                    note,
                    EventKind::Expression,
                    6,
                    value as f64 / u32::MAX as f64,
                ),
                Timbre { note, value } => (
                    note,
                    EventKind::Expression,
                    5,
                    value as f64 / u32::MAX as f64,
                ),
                Controller {
                    note,
                    controller: 74,
                    value,
                } => (
                    note,
                    EventKind::Expression,
                    5,
                    value as f64 / u32::MAX as f64,
                ),
                Controller {
                    note,
                    controller: 64,
                    ..
                } => {
                    if self.midi.zone_voices.get(&note).is_some_and(|v| {
                        matches!(self.node_defs.get(&v.id), Some(NodeDef::Plugin(..)))
                    }) {
                        return Ok(true);
                    }
                    return Ok(false);
                }
                Controller { note, .. } => {
                    if self.midi.zone_voices.get(&note).is_some_and(|v| {
                        matches!(self.node_defs.get(&v.id), Some(NodeDef::Plugin(..)))
                    }) {
                        return Err("unsupported native MPE controller/dialect".into());
                    }
                    return Ok(false);
                }
                _ => return Ok(false),
            };
        let Some(voice) = self.midi.zone_voices.get(&note).copied() else {
            return Ok(false);
        };
        if !matches!(self.node_defs.get(&voice.id), Some(NodeDef::Plugin(..))) {
            return Ok(false);
        }
        self.plugin_event_cmd(
            voice.id,
            MIDI_SOURCE | 256 | voice.master as u128,
            None,
            Event {
                kind: kind as u32,
                id,
                note_id: i32::try_from(note)
                    .map_err(|_| "native note identity capacity exceeded")?,
                channel: -1,
                key: voice.key as i16,
                value,
                ..Event::default()
            },
            cmds,
        )?;
        if kind == EventKind::NoteOff {
            self.midi.zone_voices.remove(&note);
        }
        Ok(true)
    }
}

impl CmdTranslator {
    pub(crate) fn clear_plugin_lane(&mut self, lane: i32) {
        self.plugin_note_queues
            .retain(|(node, _, source, ..), queue| {
                let keep =
                    source >> 96 != 1 || ((source >> 32) & u32::MAX as u128) != lane as u32 as u128;
                if !keep && let Some(count) = self.plugin_note_counts.get_mut(node) {
                    *count -= queue.len();
                }
                keep
            });
    }
    pub(crate) fn retire_plugin_generation(&mut self, node: i32, generation: u64) {
        self.plugin_note_queues
            .retain(|(instance, incarnation, ..), queue| {
                if *instance == node && *incarnation == generation {
                    if let Some(count) = self.plugin_note_counts.get_mut(&node) {
                        *count -= queue.len();
                    }
                    false
                } else {
                    true
                }
            });
    }
    pub(crate) fn retire_plugin_note(&mut self, node: i32, token: i32) {
        self.plugin_note_queues.retain(|(instance, ..), queue| {
            if *instance == node {
                let before = queue.len();
                queue.retain(|id| *id != token);
                if let Some(count) = self.plugin_note_counts.get_mut(&node) {
                    *count -= before - queue.len();
                }
            }
            !queue.is_empty()
        });
    }
}
