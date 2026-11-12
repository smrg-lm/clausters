//! **MIDI 2.0's Universal MIDI Packets**, read into what the actuation path
//! plays.
//!
//! A Channel Voice 2 message is a [`ChannelVoiceMessage`] at the resolution
//! the path already keeps -- a 16-bit velocity, 32-bit controllers, pressure
//! and bend -- so a note, a controller or a bend from MIDI 2.0 plays exactly
//! as one widened from MIDI 1.0 does, with nothing lost on the way. What MIDI
//! 1.0 has no message for is the **per-note** half: a pitch bend, a
//! registered or an assignable controller addressed to one note, which is
//! what MPE spends a channel per note to emulate. Those are [`PerNote`], and
//! they reach the voice sounding on their channel and key. A MIDI 1.0 message
//! carried in a packet is its bytes again.

use super::ChannelVoiceMessage;

/// A per-note pitch bend's range, in semitones, MIDI 2.0's default.
pub const PER_NOTE_BEND: f32 = 48.0;

/// What one note is told, beside its channel's messages.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PerNote {
    /// Its pitch bent this many semitones off its key.
    Bend(f32),
    /// A controller of its own, registered (whose number is a CC's meaning,
    /// 74 the timbre MPE calls brightness) or assignable, at 32 bits.
    Controller { index: u8, value: u32 },
}

/// One packet's message, as the actuation path plays it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UmpMessage {
    /// A channel voice message, at full resolution.
    Voice(ChannelVoiceMessage),
    /// A message to the note `note` of `channel`.
    PerNote {
        channel: u8,
        note: u8,
        message: PerNote,
    },
    /// A MIDI 1.0 channel voice message carried in a packet: its bytes.
    Midi1([u8; 3]),
}

/// How many words a packet of message type `mt` holds.
fn words_of(mt: u32) -> usize {
    match mt {
        0x0..=0x2 | 0x6 | 0x7 => 1,
        0x3 | 0x4 | 0x8..=0xA => 2,
        0xB | 0xC => 3,
        _ => 4,
    }
}

/// **The messages a stream of packets holds**, in order: MIDI 1.0 and 2.0
/// channel voice messages, and the per-note ones. A packet of any other kind
/// -- utility, system, data, stream -- plays nothing and is passed over, and a
/// packet cut short at the end is dropped.
pub fn parse_ump(words: &[u32]) -> Vec<UmpMessage> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let head = words[i];
        let mt = head >> 28;
        let size = words_of(mt);
        let Some(packet) = words.get(i..i + size) else {
            break;
        };
        i += size;
        let status = ((head >> 20) & 0xF) as u8;
        let channel = ((head >> 16) & 0xF) as u8;
        let index = ((head >> 8) & 0x7F) as u8;
        let extra = (head & 0xFF) as u8;
        match mt {
            0x2 => out.push(UmpMessage::Midi1([
                ((head >> 16) & 0xFF) as u8,
                ((head >> 8) & 0x7F) as u8,
                (head & 0x7F) as u8,
            ])),
            0x4 => {
                let data = packet[1];
                let message = match status {
                    // MIDI 2.0 has no note-on that means off: a velocity of 0
                    // is the softest note there is.
                    0x9 => UmpMessage::Voice(ChannelVoiceMessage::NoteOn {
                        channel,
                        note: index,
                        velocity: ((data >> 16) as u16).max(1),
                    }),
                    0x8 => UmpMessage::Voice(ChannelVoiceMessage::NoteOff {
                        channel,
                        note: index,
                        velocity: (data >> 16) as u16,
                    }),
                    0xA => UmpMessage::Voice(ChannelVoiceMessage::PolyAftertouch {
                        channel,
                        note: index,
                        pressure: data,
                    }),
                    0xB => UmpMessage::Voice(ChannelVoiceMessage::ControlChange {
                        channel,
                        controller: index,
                        value: data,
                    }),
                    0xC => UmpMessage::Voice(ChannelVoiceMessage::ProgramChange {
                        channel,
                        program: ((data >> 24) & 0x7F) as u8,
                    }),
                    0xD => UmpMessage::Voice(ChannelVoiceMessage::ChannelAftertouch {
                        channel,
                        pressure: data,
                    }),
                    0xE => UmpMessage::Voice(ChannelVoiceMessage::PitchBend {
                        channel,
                        value: data,
                    }),
                    0x6 => {
                        let off = (f64::from(data) - 2_147_483_648.0) / 2_147_483_648.0;
                        UmpMessage::PerNote {
                            channel,
                            note: index,
                            message: PerNote::Bend(off as f32 * PER_NOTE_BEND),
                        }
                    }
                    0x0 | 0x1 => UmpMessage::PerNote {
                        channel,
                        note: index,
                        message: PerNote::Controller {
                            index: extra,
                            value: data,
                        },
                    },
                    _ => continue,
                };
                out.push(message);
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Each message a packet holds**, at the resolution it came in: a note
    /// at 16 bits (a 0 velocity still a note), a controller at 32, a per-note
    /// bend in semitones, a per-note controller, a MIDI 1.0 message's bytes;
    /// the framing passed over and a packet cut short dropped.
    #[test]
    fn packets_are_the_messages_they_hold() {
        let words = [
            0x0030_01E0, // DCTPQ: framing
            0x4091_3C00,
            0x0000_0000, // a note on, velocity 0
            0x40B1_0700,
            0x8000_0000, // CC 7 at half
            0x4061_3C00,
            0xA000_0000, // per-note bend: 12 of 48 up
            0x4001_3C4A,
            0xFFFF_FFFF, // registered per-note 74, full
            0x2090_3C64, // MIDI 1.0 note on in a packet
            0x4081_3C00, // cut short
        ];
        assert_eq!(
            parse_ump(&words),
            [
                UmpMessage::Voice(ChannelVoiceMessage::NoteOn {
                    channel: 1,
                    note: 60,
                    velocity: 1
                }),
                UmpMessage::Voice(ChannelVoiceMessage::ControlChange {
                    channel: 1,
                    controller: 7,
                    value: 0x8000_0000
                }),
                UmpMessage::PerNote {
                    channel: 1,
                    note: 60,
                    message: PerNote::Bend(12.0)
                },
                UmpMessage::PerNote {
                    channel: 1,
                    note: 60,
                    message: PerNote::Controller {
                        index: 74,
                        value: u32::MAX
                    }
                },
                UmpMessage::Midi1([0x90, 60, 100]),
            ]
        );
    }
}
