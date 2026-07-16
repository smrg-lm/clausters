//! Live MIDI input painting notes into `midi_in` piano-rolls (native-only —
//! the virtual input port is a platform device). A note-on inserts a **held**
//! note; the matching note-off closes it. The note model edits go through the
//! same shared doors the gesture machine uses.

use crate::host::interact;
use crate::host::pianoroll;

use super::app::App;

impl App {
    /// Drain the virtual input port and paint each note event into every
    /// `midi_in` roll. A note-on inserts a **held** note — at the running
    /// playhead (live recording), or at the step cursor when the transport is
    /// stopped (step entry) — and the matching note-off closes it: the real
    /// held duration in playhead mode, or a grid step (advancing the cursor
    /// once all keys are up) in step mode.
    pub(super) fn drain_midi(&mut self, rolls: &[(i32, i32)]) {
        let mut events = Vec::new();
        if let Some(input) = &self.midi_in {
            while let Some(msg) = input.poll() {
                if let Some(ev) = clausters_midi::parse_note(&msg) {
                    events.push(ev);
                }
            }
        }
        if events.is_empty() {
            return;
        }
        for &(def_id, id) in rolls {
            for &ev in &events {
                self.paint_note(def_id, id, ev);
            }
            self.host.sync_track_totals();
            self.emit_notes(def_id, id);
            self.redraw(def_id);
        }
    }

    /// Paint one live note event into a roll (see [`App::drain_midi`]).
    fn paint_note(&mut self, def_id: i32, id: i32, ev: clausters_midi::NoteEvent) {
        use clausters_midi::NoteEvent;
        let playhead = self.playhead_sample(def_id, id);
        let snap = self.roll_snap(def_id, id);
        // The painted length: the note grid, else a visible sliver of the view
        // (the Ctrl+click default) — note-off then sets the real duration.
        let dur = if snap > 0.0 {
            snap
        } else {
            self.timeline_nav(id)
                .map_or(1.0, |(_, len, _)| (len * 0.05).max(1.0))
        };
        match ev {
            NoteEvent::On {
                channel,
                pitch,
                velocity,
            } => {
                let pos = match playhead {
                    Some(p) => interact::snap(p, snap).max(0.0),
                    None => *self.step.entry((def_id, id)).or_insert(0.0),
                };
                let index = interact::pianoroll_notes_edit(&mut self.host, def_id, id, |notes| {
                    pianoroll::insert_note(
                        notes,
                        pianoroll::Note {
                            start: pos,
                            dur,
                            pitch: pitch as f32,
                            velocity: velocity as i32,
                            channel: channel as i32,
                        },
                    )
                });
                if let Some(index) = index {
                    self.held.insert((def_id, id, channel, pitch), index);
                }
            }
            NoteEvent::Off { channel, pitch } => {
                let Some(index) = self.held.remove(&(def_id, id, channel, pitch)) else {
                    return;
                };
                if let Some(now) = playhead {
                    // Live recording: the key was held this long.
                    interact::pianoroll_notes_edit(&mut self.host, def_id, id, |notes| {
                        if let Some(n) = notes.get_mut(index) {
                            n.dur = (now - n.start).max(1.0);
                        }
                    });
                } else if !self
                    .held
                    .keys()
                    .any(|(d, w, _, _)| (*d, *w) == (def_id, id))
                {
                    // Step entry: the last key up advances the cursor a grid
                    // (a chord steps once).
                    *self.step.entry((def_id, id)).or_insert(0.0) += dur;
                }
            }
        }
    }
}
