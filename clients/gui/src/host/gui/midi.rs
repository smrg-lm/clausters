//! The desktop's MIDI **device**: the virtual input port (native-only -- a
//! port other programs route into is a platform device).
//!
//! The front's whole job is the port: hold it open while some window has an
//! element that reads live MIDI, and hand what comes out of it to the shared
//! walk (`crate::host::midi`), which reads the messages and delivers the notes
//! -- the same walk a page's inputs feed.

use super::app::App;

impl App {
    /// Drain the virtual input port and deliver each note to every element that
    /// reads live MIDI, reporting whatever comes back the way a gesture's edit
    /// is reported.
    pub(super) fn drain_midi(&mut self, readers: &[(i32, i32)]) {
        if let Some(input) = &self.midi_in {
            while let Some(msg) = input.poll() {
                self.midi.feed(&msg);
            }
        }
        let notes = self.midi.notes();
        let effects = self.host.deliver_midi(readers, &notes, self.shm.as_deref());
        self.apply_gesture_effects(effects);
    }
}
