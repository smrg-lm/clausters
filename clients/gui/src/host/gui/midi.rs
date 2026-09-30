//! Live MIDI input reaching the elements that asked for it (native-only -- the
//! virtual input port is a platform device).
//!
//! The front's whole job is the **device**: open the port, translate what comes
//! out of it into the platform-neutral [`MidiNote`], and hand it to every
//! element that declared [`Needs::midi`], with the one fact the element cannot
//! read for itself -- where the transport stands. What a note *does* to a
//! picture is the element's, exactly as what a key does to a field is.
//!
//! The translation runs through the shared MPE decoder (`clausters_midi::mpe`,
//! the server's own), with both zones waiting for a device to size them: an
//! MPE controller's notes arrive with their bend and are retuned as it moves,
//! so a glide is painted as one, and a plain keyboard on a channel no zone
//! covers arrives as it always did.
//!
//! [`Needs::midi`]: crate::host::widget::element::Needs::midi

use clausters_midi::NoteEvent;
use clausters_midi::mpe::MpeEvent;

use crate::host::widget::element::MidiNote;

use super::app::App;

impl App {
    /// Drain the virtual input port and deliver each note to every element that
    /// reads live MIDI, reporting whatever comes back the way a gesture's edit
    /// is reported.
    pub(super) fn drain_midi(&mut self, readers: &[(i32, i32)]) {
        let mut events = Vec::new();
        if let Some(input) = &self.midi_in {
            while let Some(msg) = input.poll() {
                self.mpe.feed(&msg);
            }
        }
        while let Some(event) = self.mpe.poll() {
            if let Some(note) = self.note_of(event) {
                events.push(note);
            }
        }
        if events.is_empty() {
            return;
        }
        for &(def_id, id) in readers {
            // The running playhead in the element's own units, or `None` for a
            // stopped transport -- the difference between recording a note and
            // entering one on a step cursor.
            let playhead = self.playhead_sample(def_id, id);
            let mut reported = false;
            for &note in &events {
                let Some(events) = self.host.element_midi(def_id, id, note, playhead) else {
                    continue;
                };
                for args in events {
                    self.emit_element(def_id, id, args);
                    reported = true;
                }
            }
            if reported {
                // What was painted moved the extent the shared axis spans.
                self.host.sync_track_totals_keeping_view();
                self.redraw(def_id);
            }
        }
    }
}

impl App {
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
                self.mpe_notes.insert(id, (channel, key));
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
                let (channel, key) = self.mpe_notes.remove(&id)?;
                Some(note(false, channel, key, 0, 0.0, false))
            }
            MpeEvent::Bend {
                note: id,
                semitones,
            } => {
                let &(channel, key) = self.mpe_notes.get(&id)?;
                Some(note(true, channel, key, 0, semitones, true))
            }
            _ => None,
        }
    }
}
