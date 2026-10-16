//! The page's half of **a window's picture written to a file**
//! (`/gui_capture`): the frame drawn offscreen through the shared path, read
//! back once the card has mapped it -- a page cannot block on the card, so it
//! is asked again on the browser's loop -- and written into the page's own
//! storage, where a path in a tab is (`docs/decisions.md`, "A path in a page
//! is its own storage").

use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

use super::*;
use crate::host::frame::Readback;
use crate::host::{captured, png};

impl WebApp {
    /// **A client asked for the picture of canvas `def` at `path`**: drawn
    /// now, read and written once the card has it, and answered then.
    pub(super) fn capture(&mut self, def: i32, path: String) {
        let readback = match self.capture_frame(def) {
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

/// One turn of the browser's loop: a timer of no delay, awaited.
async fn next_turn() -> Result<(), ()> {
    let window = web_sys::window().ok_or(())?;
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        let _ = window
            .set_timeout_with_callback_and_timeout_and_arguments_0(resolve.unchecked_ref(), 0);
    });
    JsFuture::from(promise).await.map(|_| ()).map_err(|_| ())
}
