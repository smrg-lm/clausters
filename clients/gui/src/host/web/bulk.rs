//! Where a browser element's **bulk data** comes from: a URL, fetched.
//!
//! The native front resolves a `path`/`cache` by mapping the file
//! (`mapfile::MmapLoader`, native-only); the browser has no filesystem, so
//! the same two props are URLs fetched against the page origin, and a server
//! `buffer` is pulled over the audio-server leg instead. This module is that
//! resolution: what a tree asks for ([`collect_bulk`]), the fetch itself, and
//! where the answer lands -- a GPU slot for a navigable heavy view, the tree
//! for a take drawn into the mesh.
//!
//! The rule the whole path serves is the crate's: bulk moves as bulk. A
//! minutes-long take reaches a lane as a peak pyramid over one fetch, never as
//! JSON over OSC.

use super::*;
use crate::host::widget::element::Bulk;

/// Every element's **declared** bulk resource, as fetches to start — and the
/// server buffers to pull over the client leg, which are the one resource a
/// page cannot fetch for itself.
///
/// Nothing here derives what a view wants from what it is: the element says
/// which resource and in which form (`Needs::bulk`), and this walk turns a
/// local reference into the URL a page reads it from. That is the whole of the
/// browser's half.
fn collect_bulk(
    widget: &Widget,
    owner: Option<i32>,
    buffer_refs: &mut Vec<(i32, i32)>,
    requests: &mut Vec<(i32, Bulk)>,
) {
    let id = widget.id.or(owner);
    if let (Some(id), Some(want)) = (id, widget.kind.needs().bulk) {
        match want {
            Bulk::Buffer(bufnum) => buffer_refs.push((id, bufnum)),
            want => requests.push((id, want)),
        }
    }
    for child in &widget.children {
        // A clip's body carries no id of its own: the fetch is keyed by the
        // container's, which is what the reply resolves back through.
        collect_bulk(child, id, buffer_refs, requests);
    }
}

/// Fetches one bulk URL and decodes it off the event loop, then hands the
/// result back through the proxy as [`WebEvent::BulkReady`].
pub(super) async fn fetch_bulk(host: HostId, def_id: i32, widget_id: i32, request: Bulk) {
    // A declared resource is a local reference natively and a URL here: the
    // page fetches what the tree named, which is the one platform difference in
    // the whole bulk path.
    let Some(url) = request.resource().map(|p| p.to_string_lossy().into_owned()) else {
        return; // a server buffer: the client leg's, not a fetch
    };
    let bytes = match fetch_bytes(&url).await {
        Ok(bytes) => bytes,
        Err(e) => return log(&format!("bulk fetch {url}: {e}")),
    };
    let data = match request {
        Bulk::PeakCache(_) => {
            let Some(multi) = MultiPyramid::from_bytes(&bytes) else {
                return log(&format!("bulk fetch {url}: malformed peak pyramid"));
            };
            log(&format!(
                "waveform: fetched peak cache {url} ({} samples x {} channel(s), no raw data)",
                multi.frames(),
                multi.num_channels()
            ));
            Loaded::Peaks(WaveformData::with_multi_pyramid(multi))
        }
        Bulk::Peaks {
            channels,
            base_bucket,
            ..
        } => {
            let flat = decode_f32(&bytes);
            log(&format!(
                "waveform: fetched {} samples x {channels} channel(s) from {url} (pyramids built in wasm)",
                flat.len() / channels.max(1)
            ));
            Loaded::Peaks(WaveformData::from_interleaved(&flat, channels, base_bucket))
        }
        Bulk::StftCache(_) => {
            let Some(stft) = Stft::from_bytes(&bytes) else {
                return log(&format!("bulk fetch {url}: malformed STFT cache"));
            };
            log(&format!(
                "spectrogram: fetched STFT cache {url} ({} frames x {} bins)",
                stft.n_frames(),
                stft.n_bins()
            ));
            Loaded::Stfts(vec![stft])
        }
        Bulk::Stft {
            channels,
            window_size,
            hop,
            sample_rate,
            ..
        } => {
            let flat = decode_f32(&bytes);
            let stfts = frame::stft_lanes(
                frame::deinterleave(&flat, channels),
                window_size,
                hop,
                sample_rate,
            );
            log(&format!(
                "spectrogram: fetched {} samples x {channels} channel(s) from {url} (STFT in wasm)",
                flat.len() / channels.max(1)
            ));
            Loaded::Stfts(stfts)
        }
        Bulk::Samples { channels, .. } => {
            let mut flat = decode_f32(&bytes);
            let channels = channels.max(1);
            flat.truncate(flat.len() / channels * channels);
            log(&format!(
                "plot: fetched {} samples x {channels} channel(s) from {url}",
                flat.len() / channels
            ));
            Loaded::Samples(flat.into())
        }
        // Resolved above: a buffer names no URL and never reaches the fetch.
        Bulk::Buffer(_) => return,
    };
    if let Some(proxy) = web_proxy() {
        let _ = proxy.send_event(HostEvent::To(
            host,
            WebEvent::BulkReady {
                def_id,
                widget_id,
                data,
            },
        ));
    }
}

/// One `fetch` of `url` to raw bytes (an `ArrayBuffer`), erroring on a non-2xx
/// status so a missing resource is visible instead of decoding garbage.
async fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    use wasm_bindgen_futures::JsFuture;
    let window = web_sys::window().ok_or("no window")?;
    let response = JsFuture::from(window.fetch_with_str(url))
        .await
        .map_err(|e| format!("{e:?}"))?;
    let response: web_sys::Response = response.dyn_into().map_err(|_| "not a Response")?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let buffer = JsFuture::from(response.array_buffer().map_err(|e| format!("{e:?}"))?)
        .await
        .map_err(|e| format!("{e:?}"))?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}

/// Decodes raw little-endian `f32` bytes flat (interleaved as sent) — the
/// multichannel views de-interleave downstream.
fn decode_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

impl WebApp {
    /// Starts the bulk loads of a freshly opened def: server-buffer fetches
    /// over the WS leg, and `fetch`es of every waveform/plot `path`/`cache`
    /// (URLs against the page origin in the browser).
    pub(super) fn start_bulk(&mut self, def: i32) {
        let Some(tree) = self.host.window_def(def) else {
            return;
        };
        let mut buffer_refs = Vec::new();
        let mut requests = Vec::new();
        collect_bulk(tree, None, &mut buffer_refs, &mut requests);
        for (widget_id, bufnum) in buffer_refs {
            if let Some(query) = self.fetches.want(def, widget_id, bufnum) {
                self.send_to_server(query);
            }
        }
        for (widget_id, request) in requests {
            wasm_bindgen_futures::spawn_local(fetch_bulk(self.id, def, widget_id, request));
        }
    }

    /// Places a decoded GPU-bound resource (waveform or spectrogram) on its
    /// def's canvas: a slot right away when that device is up, else stashed and
    /// replayed on `GpuReady`.
    pub(super) fn place_bulk(&mut self, def_id: i32, widget_id: i32, data: Loaded) {
        let Some(slot) = self.canvases.get_mut(&def_id) else {
            return; // the canvas was detached while the fetch was in flight
        };
        let Some(render) = slot.render.as_mut() else {
            slot.pending_bulk.push((widget_id, data));
            return;
        };
        let mut total = None;
        match data {
            Loaded::Peaks(data) => {
                let slot = frame::waveform_slot(data, &render.gpu);
                total = Some(slot.view.total_samples());
                render.waveforms.insert(widget_id, slot);
            }
            Loaded::Stfts(stfts) => {
                if let Some(slot) = frame::spectrogram_slot(stfts, &render.gpu, &render.renderers) {
                    total = Some(slot.total_samples());
                    render.spectrograms.insert(widget_id, slot);
                }
            }
            // A slot takes what its kind asked for; the forms an element takes
            // home never reach here (`on_bulk_ready` routes on the declaration).
            Loaded::Samples(_) | Loaded::Raw { .. } => log(&format!(
                "widget {widget_id}: raw samples cannot fill a GPU slot"
            )),
        }
        // The loaded extent joins the widget's navigation group.
        if let Some(total) = total {
            self.host.set_timeline_total(widget_id, total);
        }
    }

    /// A fetched bulk resource arrived: an element that claimed a **GPU slot**
    /// is fed through it, and every other one takes the data home itself.
    ///
    /// The fork is the *declaration*, not the presentation — the loader knows
    /// nothing about what a signal is, exactly as the native one does not.
    pub(super) fn on_bulk_ready(&mut self, def: i32, widget_id: i32, data: Loaded) {
        let wants_slot = self
            .host
            .window_def(def)
            .and_then(|t| t.find(widget_id))
            .is_some_and(|w| slot_target(w).is_some());
        if wants_slot {
            self.place_bulk(def, widget_id, data);
        } else if let Some(widget) = self
            .host
            .window_def_mut(def)
            .and_then(|t| t.find_mut(widget_id))
        {
            take_bulk(widget, data);
        }
        self.request_redraw(def);
    }
}

/// The widget a slot is keyed by, when this one (or a body of it) claimed one —
/// a clip's body carries no id, so the slot is the container's.
fn slot_target(widget: &Widget) -> Option<&Widget> {
    if widget.kind.needs().slot.is_some() {
        return Some(widget);
    }
    widget
        .children
        .iter()
        .find(|c| c.kind.needs().slot.is_some())
}

/// Hands a loaded resource to the widget that wanted it, reaching a body when
/// the id named its container.
fn take_bulk(widget: &mut Widget, data: Loaded) -> bool {
    if widget.kind.take_bulk(data) {
        return true;
    }
    false
}
