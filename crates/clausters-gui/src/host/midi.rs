//! Live MIDI input reaching the elements that asked for it.
//!
//! Three steps, and only the first is a front's: a **device** hands over the
//! bytes of a message (a virtual input port on the desktop, Web MIDI in a
//! page), the [`Reader`] turns them into the platform-neutral [`MidiNote`],
//! and [`Host::deliver_midi`] hands each note to every element that declared
//! [`Needs::midi`], with the one fact the element cannot read for itself --
//! where the transport stands. What a note *does* to a picture is the
//! element's, exactly as what a key does to a field is.
//!
//! The translation runs through the shared MPE decoder (`clausters_midi::mpe`,
//! the server's own), with both zones waiting for a device to size them: an
//! MPE controller's notes arrive with their bend and are retuned as it moves,
//! so a glide is painted as one, and a plain keyboard on a channel no zone
//! covers arrives as it always did.
//!
//! Nothing here names a port, so nothing here differs between the two fronts:
//! a roll that listens paints the same note from the same bytes on both.
//!
//! [`Needs::midi`]: crate::host::widget::element::Needs::midi

use std::collections::HashMap;

use clausters_midi::NoteEvent;
use clausters_midi::mpe::{Decoder, MpeEvent, Side};

use super::gestures::GestureEffect;
use super::timeline::group_key;
use super::widget::element::MidiNote;
use super::{BusSource, Host};

/// What a front's input goes through: the bytes of the messages in, the notes
/// they are out.
pub struct Reader {
    /// The shared MPE decoder, both zones waiting for a device to size them.
    mpe: Decoder,
    /// A zone note's channel and key, by the decoder's note id, while it
    /// sounds.
    notes: HashMap<u32, (u8, u8)>,
}

impl Default for Reader {
    fn default() -> Self {
        let mut mpe = Decoder::new();
        mpe.set_zone(Side::Lower, Some(0));
        mpe.set_zone(Side::Upper, Some(0));
        Self {
            mpe,
            notes: HashMap::new(),
        }
    }
}

impl Reader {
    /// Takes the bytes of one message, as the device delivered them.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.mpe.feed(bytes);
    }

    /// Every note the messages fed so far amount to, in order, and none of
    /// them twice.
    pub fn notes(&mut self) -> Vec<MidiNote> {
        let mut out = Vec::new();
        while let Some(event) = self.mpe.poll() {
            out.extend(self.note_of(event));
        }
        out
    }

    /// A decoded message as the host's note event: a plain note-on or off, or
    /// a zone note's start, end or retune (whose channel and key are
    /// remembered by note id from its start). Anything else paints nothing.
    fn note_of(&mut self, event: MpeEvent) -> Option<MidiNote> {
        let note = |on, channel: u8, key: u8, velocity: i32, bend, retune| MidiNote {
            on,
            channel: i32::from(channel),
            pitch: i32::from(key),
            velocity,
            bend,
            retune,
        };
        match event {
            MpeEvent::Plain(bytes) => match clausters_midi::parse_note(&bytes)? {
                NoteEvent::On {
                    channel,
                    pitch,
                    velocity,
                } => Some(note(true, channel, pitch, i32::from(velocity), 0.0, false)),
                NoteEvent::Off { channel, pitch } => {
                    Some(note(false, channel, pitch, 0, 0.0, false))
                }
            },
            MpeEvent::NoteOn {
                note: id,
                channel,
                key,
                velocity,
                bend,
                ..
            } => {
                self.notes.insert(id, (channel, key));
                Some(note(
                    true,
                    channel,
                    key,
                    i32::from(velocity >> 9),
                    bend,
                    false,
                ))
            }
            MpeEvent::NoteOff { note: id, .. } => {
                let (channel, key) = self.notes.remove(&id)?;
                Some(note(false, channel, key, 0, 0.0, false))
            }
            MpeEvent::Bend {
                note: id,
                semitones,
            } => {
                let &(channel, key) = self.notes.get(&id)?;
                Some(note(true, channel, key, 0, semitones, true))
            }
            _ => None,
        }
    }
}

impl Host {
    /// Every element of the windows `defs` that **declared** it reads live
    /// MIDI, as `(window, widget)` -- what a front opens its input for, and
    /// closes it when there is none.
    pub fn midi_readers(&self, defs: impl IntoIterator<Item = i32>) -> Vec<(i32, i32)> {
        let mut out = Vec::new();
        for def_id in defs {
            let Some(tree) = self.window_def(def_id) else {
                continue;
            };
            out.extend(
                tree.descendants()
                    .filter_map(|w| w.kind.needs().midi.then_some((def_id, w.id?))),
            );
        }
        out
    }

    /// The shared playhead's current sample for a widget while it is running
    /// (`playhead_at` anchored to the engine clock, read off `bus`), else
    /// `None`. It is the widget's navigation group that is running or not --
    /// the recording keeps time with what the lanes draw, which is the group's
    /// sweep.
    pub fn playhead_sample(
        &self,
        def_id: i32,
        id: i32,
        bus: Option<&dyn BusSource>,
    ) -> Option<f64> {
        let tree = self.window_def(def_id)?;
        let e = tree.find(id)?.kind.editor()?;
        let clock = self.head_clocks(def_id, bus).at(Some(id));
        self.timelines()
            .state(group_key(id, e.link))?
            .swept_at(clock)
    }

    /// Delivers `notes` to every one of `readers`, and answers with what the
    /// front has left to do about it: the events the elements reported, by the
    /// rule a gesture's edit follows (a bound widget forwarded, an unbound one
    /// emitted and stamped -- live MIDI painting reports the same payloads a
    /// hand does, and the owner has no way to tell them apart), and a repaint
    /// of each window something was painted in.
    pub fn deliver_midi(
        &mut self,
        readers: &[(i32, i32)],
        notes: &[MidiNote],
        bus: Option<&dyn BusSource>,
    ) -> Vec<GestureEffect> {
        let mut out = Vec::new();
        if notes.is_empty() {
            return out;
        }
        for &(def_id, id) in readers {
            // The running playhead in the element's own units, or `None` for a
            // stopped transport -- the difference between recording a note and
            // entering one on a step cursor.
            let playhead = self.playhead_sample(def_id, id, bus);
            let mut reported = false;
            for &note in notes {
                let Some(events) = self.element_midi(def_id, id, note, playhead) else {
                    continue;
                };
                for args in events {
                    super::gestures::report(self, &mut out, def_id, id, args);
                    reported = true;
                }
            }
            if reported {
                // What was painted moved the extent the shared axis spans.
                self.sync_track_totals_keeping_view();
                out.push(GestureEffect::Redraw(def_id));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use clausters_core::osc::{OscMessage, OscPacket, OscType};

    use super::*;
    use crate::host::{ClientId, GUI_DEF};

    /// A host with one window holding a roll that listens (10) and one that
    /// does not (11).
    fn host() -> Host {
        let mut host = Host::new();
        let from = ClientId::Udp(std::net::SocketAddr::from((
            std::net::Ipv4Addr::LOCALHOST,
            9000,
        )));
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: GUI_DEF.into(),
                args: vec![
                    OscType::Int(1),
                    OscType::String(
                        r#"{"type":"window","children":[
                            {"id":10,"type":"notes","midi_in":true,"snap":100.0},
                            {"id":11,"type":"notes"}]}"#
                            .into(),
                    ),
                ],
            }),
            from,
        );
        host
    }

    /// The tags of what was emitted, by widget.
    fn emitted(effects: &[GestureEffect]) -> Vec<(i32, String)> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                GestureEffect::Emit {
                    widget_id, args, ..
                } => match args.first() {
                    Some(OscType::String(tag)) => Some((*widget_id, tag.clone())),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    #[test]
    fn only_an_element_that_declared_it_reads_midi_is_a_reader() {
        let host = host();
        assert_eq!(host.midi_readers([1]), vec![(1, 10)]);
        assert!(
            host.midi_readers([2]).is_empty(),
            "a window that is not there"
        );
    }

    /// **The bytes of a device are the note a roll paints**, whichever front
    /// read them off which device: a key pressed and released is one note on
    /// the step cursor, reported as a hand's edit is and repainted.
    #[test]
    fn a_key_played_is_a_note_painted_and_reported() {
        let mut host = host();
        let mut reader = Reader::default();
        let readers = host.midi_readers([1]);

        reader.feed(&[0x90, 60, 100]);
        let on = host.deliver_midi(&readers, &reader.notes(), None);
        reader.feed(&[0x80, 60, 0]);
        let off = host.deliver_midi(&readers, &reader.notes(), None);

        let said: Vec<_> = emitted(&on).into_iter().chain(emitted(&off)).collect();
        assert!(
            said.contains(&(10, "notes".to_string())),
            "the roll reported its notes: {said:?}"
        );
        assert!(said.iter().all(|(id, _)| *id == 10), "and only that roll");
        assert!(
            on.iter()
                .chain(&off)
                .any(|e| *e == GestureEffect::Redraw(1)),
            "the window is repainted"
        );
        assert!(reader.notes().is_empty(), "a note is delivered once");
    }

    /// A note-on of velocity 0 is a note-off, and a message that is no note
    /// paints nothing.
    #[test]
    fn the_reader_reads_notes_and_nothing_else() {
        let mut reader = Reader::default();
        reader.feed(&[0x91, 64, 90]);
        reader.feed(&[0x91, 64, 0]);
        reader.feed(&[0xB1, 7, 100]);
        let notes = reader.notes();
        assert_eq!(notes.len(), 2);
        assert!(notes[0].on && !notes[1].on);
        assert_eq!(
            (notes[0].channel, notes[0].pitch, notes[0].velocity),
            (1, 64, 90)
        );
    }

    /// Nothing played is nothing done: no effect, and no window repainted.
    #[test]
    fn no_notes_is_no_effect() {
        let mut host = host();
        let readers = host.midi_readers([1]);
        assert!(host.deliver_midi(&readers, &[], None).is_empty());
    }
}
