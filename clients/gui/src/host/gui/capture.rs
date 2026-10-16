//! **A window's picture written to a file** (`/gui_capture`): the frame drawn
//! offscreen through the shared path ([`crate::host::frame::capture`]), read back, written
//! as a PNG, and answered.

use crate::host::{ClientId, captured, png};

use super::app::{App, Asked};

impl App {
    /// **A client asked for the picture of window `def_id` at `path`**: drawn,
    /// written, and answered with `/gui_captured` -- with the reason, when the
    /// file could not be written. A window still opening is captured once it
    /// is open, after whatever was asked of it before.
    pub(super) fn capture(&mut self, def_id: i32, path: String, from: ClientId) {
        if !self.windows.contains_key(&def_id) && self.pending.iter().any(|(id, _)| *id == def_id) {
            self.pending_asks
                .push((def_id, Asked::Capture { path, from }));
            return;
        }
        let written = self.write_capture(def_id, &path);
        if let Err(e) = &written {
            tracing::warn!("/gui_capture {def_id} {path}: {e}");
        }
        self.send(from, captured(def_id, &path, written.err().as_deref()));
    }

    fn write_capture(&mut self, def_id: i32, path: &str) -> Result<(), String> {
        let readback = self.capture_frame(def_id)?;
        let ws = self
            .windows
            .get(&def_id)
            .ok_or("no window is open for it")?;
        readback.wait(&ws.gpu.device)?;
        let (width, height, rgba) = readback.pixels();
        std::fs::write(path, png::encode_rgba(width, height, &rgba)).map_err(|e| e.to_string())
    }
}
