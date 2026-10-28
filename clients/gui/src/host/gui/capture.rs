//! **A window's picture written to a file** (`/gui_capture`): the frame drawn
//! offscreen through the shared path ([`crate::host::frame::capture`]), read back, written
//! as a PNG, and answered.

use std::time::{Duration, Instant};

use crate::host::replies::Front;
use crate::host::{ClientId, captured, png};

use super::app::{App, Asked};

/// How long a capture waits for its window's picture to be read before it
/// is written as it stands.
pub(super) const CAPTURE_WAIT: Duration = Duration::from_secs(10);

impl App {
    /// **A client asked for the picture of window `def_id` at `path`**: drawn,
    /// written, and answered with `/gui_captured` -- with the reason, when the
    /// file could not be written. A window still opening is captured once it
    /// is open, after whatever was asked of it before.
    ///
    /// **A capture asks for what its picture needs.** A window the compositor
    /// does not draw receives no frame, so nothing would note what its views
    /// owe: the capture's own draw notes it, the reads are asked for at once,
    /// and the picture is written once nothing more is owed
    /// ([`Front::awaits_picture`]), or after [`CAPTURE_WAIT`].
    pub(super) fn capture(&mut self, def_id: i32, path: String, from: ClientId) {
        if !self.windows.contains_key(&def_id) && self.pending.iter().any(|(id, _)| *id == def_id) {
            self.pending_asks
                .push((def_id, Asked::Capture { path, from }));
            return;
        }
        self.capturing
            .push((def_id, path, from, Instant::now() + CAPTURE_WAIT));
        self.finish_captures();
    }

    /// **The captures whose picture is read**, drawn and written; the others
    /// wait. A window's picture is drawn first, which notes what it owes, and
    /// what it owes is asked for before deciding.
    pub(super) fn finish_captures(&mut self) {
        let now = Instant::now();
        for (def_id, path, from, until) in std::mem::take(&mut self.capturing) {
            if !self.windows.contains_key(&def_id) {
                self.send(from, captured(def_id, &path, Some("the window closed")));
                continue;
            }
            if now < until && self.awaits_picture(def_id) {
                self.capturing.push((def_id, path, from, until));
                continue;
            }
            // the draw notes what the picture owes; it is asked for at once
            let drawn = self.capture_frame(def_id);
            self.ask_owed_spans(&[def_id]);
            if now < until && self.awaits_picture(def_id) {
                self.capturing.push((def_id, path, from, until));
                continue;
            }
            let written = drawn.and_then(|readback| self.write_readback(def_id, &path, readback));
            if let Err(e) = &written {
                tracing::warn!("/gui_capture {def_id} {path}: {e}");
            }
            self.send(from, captured(def_id, &path, written.err().as_deref()));
        }
    }

    fn write_readback(
        &mut self,
        def_id: i32,
        path: &str,
        readback: crate::host::frame::Readback,
    ) -> Result<(), String> {
        let ws = self
            .windows
            .get(&def_id)
            .ok_or("no window is open for it")?;
        readback.wait(&ws.gpu.device)?;
        let (width, height, rgba) = readback.pixels();
        std::fs::write(path, png::encode_rgba(width, height, &rgba)).map_err(|e| e.to_string())
    }
}
