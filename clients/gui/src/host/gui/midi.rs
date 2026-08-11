//! Live MIDI input reaching the elements that asked for it (native-only — the
//! virtual input port is a platform device).
//!
//! The front's whole job is the **device**: open the port, translate what comes
//! out of it into the platform-neutral [`MidiNote`], and hand it to every
//! element that declared [`Needs::midi`], with the one fact the element cannot
//! read for itself — where the transport stands. What a note *does* to a
//! picture is the element's, exactly as what a key does to a field is.
//!
//! [`Needs::midi`]: crate::host::widget::element::Needs::midi

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
                if let Some(ev) = clausters_midi::parse_note(&msg) {
                    events.push(note_of(ev));
                }
            }
        }
        if events.is_empty() {
            return;
        }
        for &(def_id, id) in readers {
            // The running playhead in the element's own units, or `None` for a
            // stopped transport — the difference between recording a note and
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

/// The shared crate's channel-voice note event as the host's own — the same
/// translation the keyboard front does for a key, so an element answers
/// identically wherever it is compiled.
fn note_of(ev: clausters_midi::NoteEvent) -> MidiNote {
    match ev {
        clausters_midi::NoteEvent::On {
            channel,
            pitch,
            velocity,
        } => MidiNote {
            on: true,
            channel: channel as i32,
            pitch: pitch as i32,
            velocity: velocity as i32,
        },
        clausters_midi::NoteEvent::Off { channel, pitch } => MidiNote {
            on: false,
            channel: channel as i32,
            pitch: pitch as i32,
            velocity: 0,
        },
    }
}
