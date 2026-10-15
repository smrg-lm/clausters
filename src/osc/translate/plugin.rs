//! Native events resolve stable ports off RT, then target one persistent node.
use super::*;
use clausters_core::plugin_shm::{Event, EventKind};
use serde::Deserialize;

/// Client origins are distinct from reserved live-MIDI and lane origins.
pub(super) type MidiAddress = (i32, u64, u128, i16, i16, i16);

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
