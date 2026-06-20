//! MIDI: standard channel-voice messages as a node/control actuation path
//! (M17).
//!
//! Standard MIDI — note on/off, velocity, aftertouch, pitch-bend, control
//! change, program change — is the **primary** way to drive synthesis nodes
//! and their named `f32` input controls from a sequencer, the interoperable
//! path any DAW or controller speaks. (SysEx, when it lands, is reserved for
//! the non-musical control plane — SynthDef/FaustDef load, buffers, topology —
//! never a tunnel for every OSC command.)
//!
//! This module is **transport-independent**: it parses/represents a decoded
//! [`ChannelVoiceMessage`] and holds the binding state ([`MidiBindings`]); the
//! mapping to engine commands lives in [`crate::osc::translate::CmdTranslator`]
//! (`translate_midi`), which synthesizes the equivalent `/s_new` / `/n_set` /
//! `/n_free` and reuses the OSC path, so a MIDI-driven voice is byte-identical
//! to the OSC one. The wire transport (how UMP/MIDI bytes arrive — UDP MIDI
//! 2.0, ALSA seq, or a virtual port) is the remaining open decision; see
//! `PLAN.md` M17. All of this runs on the network thread, never the audio
//! thread.
//!
//! **Backward compatibility**: messages are normalized to MIDI 2.0 / UMP
//! resolution (16-bit velocity, 32-bit controllers/pressure/bend); classic
//! MIDI 1.0 7/14-bit input is accepted and widened up to those (see
//! [`parse_midi1`]), so the same `f32` zones are driven either way.

use std::collections::HashMap;

pub mod convert;
#[cfg(feature = "midi")]
pub mod live;

/// MIDI-spawned voices get node IDs from a reserved range, disjoint from the
/// client ID space and the `/s_new -1` auto range (`AUTO_NODE_ID_BASE`).
pub const MIDI_NODE_ID_BASE: i32 = 3_000_000;

/// A decoded standard channel-voice message, normalized to MIDI 2.0 / UMP
/// resolution. One variant per message type the actuation path handles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChannelVoiceMessage {
    NoteOn {
        channel: u8,
        note: u8,
        velocity: u16,
    },
    NoteOff {
        channel: u8,
        note: u8,
        velocity: u16,
    },
    /// Per-note (polyphonic) aftertouch.
    PolyAftertouch {
        channel: u8,
        note: u8,
        pressure: u32,
    },
    /// Channel pressure (mono aftertouch).
    ChannelAftertouch {
        channel: u8,
        pressure: u32,
    },
    ControlChange {
        channel: u8,
        controller: u8,
        value: u32,
    },
    ProgramChange {
        channel: u8,
        program: u8,
    },
    PitchBend {
        channel: u8,
        value: u32,
    },
}

/// Widen a 7-bit MIDI 1.0 value to 16 bits (bit-repeat fill, so 0→0 and
/// 127→65535).
#[inline]
pub fn widen_7_to_16(v: u8) -> u16 {
    let v = (v & 0x7f) as u16;
    (v << 9) | (v << 2) | (v >> 5)
}

/// Widen a 7-bit MIDI 1.0 value to 32 bits (bit-repeat fill).
#[inline]
pub fn widen_7_to_32(v: u8) -> u32 {
    let v = (v & 0x7f) as u32;
    (v << 25) | (v << 18) | (v << 11) | (v << 4) | (v >> 3)
}

/// Widen a 14-bit MIDI 1.0 value (e.g. pitch bend) to 32 bits.
#[inline]
pub fn widen_14_to_32(v: u16) -> u32 {
    let v = (v & 0x3fff) as u32;
    (v << 18) | (v << 4) | (v >> 10)
}

/// Decode one classic MIDI 1.0 channel-voice message from a status byte and up
/// to two data bytes, widening to MIDI 2.0 resolution. Returns `None` for
/// non-channel-voice or malformed input. (A provisional helper for testing and
/// for a future MIDI-1.0-capable transport; the canonical wire form is UMP.)
pub fn parse_midi1(status: u8, data1: u8, data2: u8) -> Option<ChannelVoiceMessage> {
    let channel = status & 0x0f;
    match status & 0xf0 {
        0x80 => Some(ChannelVoiceMessage::NoteOff {
            channel,
            note: data1 & 0x7f,
            velocity: widen_7_to_16(data2),
        }),
        0x90 => {
            // Note-on with velocity 0 is a note-off, by convention.
            if data2 == 0 {
                Some(ChannelVoiceMessage::NoteOff {
                    channel,
                    note: data1 & 0x7f,
                    velocity: 0,
                })
            } else {
                Some(ChannelVoiceMessage::NoteOn {
                    channel,
                    note: data1 & 0x7f,
                    velocity: widen_7_to_16(data2),
                })
            }
        }
        0xa0 => Some(ChannelVoiceMessage::PolyAftertouch {
            channel,
            note: data1 & 0x7f,
            pressure: widen_7_to_32(data2),
        }),
        0xb0 => Some(ChannelVoiceMessage::ControlChange {
            channel,
            controller: data1 & 0x7f,
            value: widen_7_to_32(data2),
        }),
        0xc0 => Some(ChannelVoiceMessage::ProgramChange {
            channel,
            program: data1 & 0x7f,
        }),
        0xd0 => Some(ChannelVoiceMessage::ChannelAftertouch {
            channel,
            pressure: widen_7_to_32(data1),
        }),
        0xe0 => Some(ChannelVoiceMessage::PitchBend {
            channel,
            value: widen_14_to_32(((data2 as u16) << 7) | (data1 as u16 & 0x7f)),
        }),
        _ => None,
    }
}

/// How one MIDI channel actuates nodes: which instrument def to instantiate
/// per note, where, and which control each expressive message drives. Defaults
/// match the client `Event` convention (`freq`/`amp`); `/midi_map` overrides
/// or adds entries.
#[derive(Clone)]
pub struct MidiBinding {
    /// Instrument def name (SynthDef *or* FaustDef — actuated identically).
    pub instrument: String,
    pub target: i32,
    pub action: i32,
    /// Note off sets `gate_control` to 0 (gate-aware defs) instead of `/n_free`.
    pub gate: bool,
    /// Control names per message type (overridable via `/midi_map`).
    pub freq_control: String,
    pub amp_control: String,
    pub gate_control: String,
    pub bend_control: Option<String>,
    /// Channel pressure (mono aftertouch).
    pub pressure_control: Option<String>,
    /// Poly (per-note) aftertouch.
    pub poly_control: Option<String>,
    /// CC number → control name.
    pub cc: HashMap<u8, String>,
    /// Program number → instrument def name (program change re-selects it).
    pub programs: HashMap<u8, String>,
    /// M18: when the instrument is a **GraphDef**, the shared instance group
    /// spawned at bind time. A note then spawns a per-voice sub-graph
    /// (`/graph_voice`) inside it instead of a plain `/s_new`.
    pub graph_instance: Option<i32>,
}

impl MidiBinding {
    pub fn new(instrument: String, target: i32, action: i32, gate: bool) -> Self {
        Self {
            instrument,
            target,
            action,
            gate,
            freq_control: "freq".into(),
            amp_control: "amp".into(),
            gate_control: "gate".into(),
            bend_control: None,
            pressure_control: None,
            poly_control: None,
            cc: HashMap::new(),
            programs: HashMap::new(),
            graph_instance: None,
        }
    }
}

/// The server's MIDI binding state: per-channel bindings, the live
/// `(channel, note) → node` voice table, and the reserved node-ID allocator.
/// Lives on the network thread.
#[derive(Default)]
pub struct MidiBindings {
    pub channels: HashMap<u8, MidiBinding>,
    pub voices: HashMap<(u8, u8), i32>,
    next_id: i32,
}

impl MidiBindings {
    pub fn new() -> Self {
        Self {
            channels: HashMap::new(),
            voices: HashMap::new(),
            next_id: MIDI_NODE_ID_BASE,
        }
    }

    /// Allocate the next node ID for a MIDI-spawned voice.
    pub fn alloc_id(&mut self) -> i32 {
        self.next_id += 1;
        self.next_id
    }

    /// Drop all voices of a channel (on unbind), returning their node IDs.
    pub fn drain_channel(&mut self, channel: u8) -> Vec<i32> {
        let ids: Vec<((u8, u8), i32)> = self
            .voices
            .iter()
            .filter(|((c, _), _)| *c == channel)
            .map(|(k, v)| (*k, *v))
            .collect();
        for (k, _) in &ids {
            self.voices.remove(k);
        }
        ids.into_iter().map(|(_, id)| id).collect()
    }

    /// Node IDs of every live voice on a channel (for channel-wide messages).
    pub fn voice_ids(&self, channel: u8) -> Vec<i32> {
        self.voices
            .iter()
            .filter(|((c, _), _)| *c == channel)
            .map(|(_, &id)| id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_on_velocity0_is_note_off() {
        assert!(matches!(
            parse_midi1(0x90, 60, 0),
            Some(ChannelVoiceMessage::NoteOff { note: 60, .. })
        ));
        assert!(matches!(
            parse_midi1(0x90, 60, 100),
            Some(ChannelVoiceMessage::NoteOn { note: 60, .. })
        ));
    }

    #[test]
    fn widening_hits_full_scale() {
        assert_eq!(widen_7_to_16(0), 0);
        assert_eq!(widen_7_to_16(127), u16::MAX);
        assert_eq!(widen_7_to_32(0), 0);
        assert_eq!(widen_7_to_32(127), u32::MAX);
    }

    #[test]
    fn pitch_bend_center() {
        // 14-bit center is 0x2000 → ~0x8000_0000 widened.
        if let Some(ChannelVoiceMessage::PitchBend { value, .. }) = parse_midi1(0xe0, 0x00, 0x40) {
            assert!(convert::bend2control(value).abs() < 1e-3);
        } else {
            panic!("expected pitch bend");
        }
    }
}
