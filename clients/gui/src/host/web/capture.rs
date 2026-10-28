//! The page's half of **a window's picture written to a file**
//! (`/gui_capture`): the frame drawn offscreen through the shared path, read
//! back once the card has mapped it -- a page cannot block on the card, so it
//! is asked again on the browser's loop -- and written into the page's own
//! storage, where a path in a tab is (`docs/decisions.md`, "A path in a page
//! is its own storage").

use super::*;
use crate::host::frame::Readback;
use crate::host::replies::Front;
use crate::host::{captured, png};

/// How long a capture waits for its canvas' picture to be read before it is
/// written as it stands, in milliseconds.
const CAPTURE_WAIT_MS: f64 = 10_000.0;

impl WebApp {
    /// **A client asked for the picture of canvas `def` at `path`**: drawn,
    /// read and written once the card has it, and answered then. As natively,
    /// **a capture asks for what its picture needs**: its draw notes what the
    /// views owe, the reads are asked for at once, and it is written once
    /// nothing more is owed ([`Front::awaits_picture`]), or after
    /// [`CAPTURE_WAIT_MS`].
    pub(super) fn capture(&mut self, def: i32, path: String) {
        let until = canvas::now_ms() + CAPTURE_WAIT_MS;
        self.capturing.push((def, path, until));
        self.finish_captures();
    }

    /// **The captures whose picture is read**, drawn and written; the others
    /// wait for the next event.
    pub(super) fn finish_captures(&mut self) {
        if self.capturing.is_empty() {
            return;
        }
        let now = canvas::now_ms();
        for (def, path, until) in std::mem::take(&mut self.capturing) {
            if now < until && self.awaits_picture(def) {
                self.capturing.push((def, path, until));
                continue;
            }
            let readback = self.capture_frame(def);
            self.ask_owed_spans(&[def]);
            if now < until && self.awaits_picture(def) {
                self.capturing.push((def, path, until));
                continue;
            }
            self.write_capture(def, path, readback);
        }
    }

    fn write_capture(&mut self, def: i32, path: String, readback: Result<Readback, String>) {
        let readback = match readback {
            Ok(readback) => readback,
            Err(e) => {
                log(&format!("/gui_capture {def} {path}: {e}"));
                return self.queue(captured(def, &path, Some(&e)));
            }
        };
        let Some(device) = self
            .canvases
            .get(&def)
            .and_then(|slot| slot.render.as_ref())
            .map(|render| render.gpu.device.clone())
        else {
            return self.queue(captured(def, &path, Some("no canvas holds that window")));
        };
        wasm_bindgen_futures::spawn_local(finish(self.id, def, path, readback, device));
    }
}

/// Waits for the card to map the picture, writes it and hands the answer back
/// through the proxy.
async fn finish(host: HostId, def_id: i32, path: String, readback: Readback, device: wgpu::Device) {
    let result = loop {
        // A WebGL2 card answers only when it is polled; a WebGPU one on the
        // browser's loop, where this poll does nothing.
        let _ = device.poll(wgpu::PollType::Poll);
        if let Some(mapped) = readback.ready() {
            break mapped;
        }
        if next_turn().await.is_err() {
            break Err("the page has no timer to wait on the card with".into());
        }
    };
    let result = match result {
        Ok(()) => {
            let (width, height, rgba) = readback.pixels();
            files::write_opfs(&path, &png::encode_rgba(width, height, &rgba)).await
        }
        Err(e) => Err(e),
    };
    if let Err(e) = &result {
        log(&format!("/gui_capture {def_id} {path}: {e}"));
    }
    if let Some(proxy) = web_proxy() {
        let _ = proxy.send_event(HostEvent::To(
            host,
            WebEvent::Captured {
                def_id,
                path,
                result,
            },
        ));
    }
}
