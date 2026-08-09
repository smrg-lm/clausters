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

/// A fetched-and-decoded bulk resource, ready to place. The decode (pyramid
/// mapping, raw-`f32` de-interleave, in-wasm pyramid/STFT build) happens in
/// the async fetch task; placing a waveform/spectrogram needs the GPU, a plot
/// only the tree.
pub(super) enum BulkData {
    Waveform(WaveformData),
    Spectrogram(Vec<Stft>),
    Plot(Arc<[f32]>),
}

/// One waveform/spectrogram/plot URL to fetch and how to decode its bytes.
pub(super) enum BulkRequest {
    /// A prebuilt peak-pyramid cache (mono v1 or multichannel v2), mapped
    /// straight to a [`MultiPyramid`].
    Cache(String),
    /// Raw little-endian `f32`: de-interleave every channel, build the
    /// pyramids in wasm (the analysis lives in `clausters-core`, FFI-free).
    Raw {
        url: String,
        channels: usize,
        base_bucket: usize,
    },
    /// A prebuilt (single-channel) STFT cache for a `spectrogram`.
    StftCache(String),
    /// Raw little-endian `f32` for a `spectrogram`: de-interleave every
    /// channel and analyze each in wasm.
    StftRaw {
        url: String,
        channels: usize,
        window_size: usize,
        hop: usize,
        sample_rate: f64,
    },
    /// Raw little-endian `f32` for a `plot` (kept interleaved, no pyramid).
    Plot { url: String, channels: usize },
}

/// Builds the GPU slot for every inline-data `waveform`/`spectrogram` in the
/// tree (the zero-latency bulk source; `path`/`cache`/`buffer` references load
/// async through [`fetch_bulk`] and the fetch machine).
pub(super) fn build_inline_timelines(
    widget: &Widget,
    owner: Option<i32>,
    gpu: &Gpu,
    renderers: &Renderers,
    waveforms: &mut HashMap<i32, WaveformSlot>,
    spectrograms: &mut HashMap<i32, SpectrogramSlot>,
) {
    frame::visit_elements(widget, owner, &mut |owner, el| {
        // Only what is already here: an element still naming a `path`/`cache`/
        // `buffer` has empty samples at this point and is the fetch machine's,
        // so building its slot now would show an empty view until the data
        // lands and replaces it.
        if let (Some(id), true) = (owner, el.needs_gpu_slot())
            && let Some(data) = el.source.data().filter(|d| !d.samples.is_empty())
        {
            frame::inline_slot(id, el, data, gpu, renderers, waveforms, spectrograms);
        }
    });
}

/// Walks the tree collecting the async bulk sources: waveforms referencing a
/// server `buffer` (fetched over the WS leg) and waveform/plot `path`/`cache`
/// references (URLs fetched against the page origin). The browser mirror of
/// the native front's mapped-file resolution (`collect_timelines` and
/// `load_element_bulk` there), minus the inline case, which is
/// [`build_inline_timelines`] over the shared walk.
fn collect_bulk(
    widget: &Widget,
    owner: Option<i32>,
    buffer_refs: &mut Vec<(i32, i32)>,
    requests: &mut Vec<(i32, BulkRequest)>,
) {
    frame::visit_elements(widget, owner, &mut |owner, el| {
        let (Some(id), Some(data)) = (owner, el.source.data()) else {
            return;
        };
        let time_freq = el.presentation == Presentation::TimeFrequency;
        if !el.needs_gpu_slot() && data.bulk {
            // A **take** drawn into the mesh (a clip's body): the same
            // resolution a heavy view's samples take — cache, then
            // path, then buffer — landing in the tree, not the GPU.
            if let Some(cache) = &data.cache {
                requests.push((id, BulkRequest::Cache(cache.to_string_lossy().into_owned())));
            } else if let Some(path) = &data.path {
                requests.push((
                    id,
                    BulkRequest::Raw {
                        url: path.to_string_lossy().into_owned(),
                        channels: data.channels,
                        base_bucket: data.base_bucket,
                    },
                ));
            } else if let (Some(bufnum), true) = (data.buffer, data.is_empty()) {
                buffer_refs.push((id, bufnum));
            }
        } else if !el.needs_gpu_slot() {
            // A **sequence**: its samples go straight into the tree —
            // no pyramid, no analysis cache to fetch.
            if data.samples.is_empty()
                && let Some(path) = &data.path
            {
                requests.push((
                    id,
                    BulkRequest::Plot {
                        url: path.to_string_lossy().into_owned(),
                        channels: data.channels,
                    },
                ));
            }
        } else if let Some(cache) = &data.cache {
            let url = cache.to_string_lossy().into_owned();
            requests.push((
                id,
                if time_freq {
                    BulkRequest::StftCache(url)
                } else {
                    BulkRequest::Cache(url)
                },
            ));
        } else if let Some(path) = &data.path {
            let url = path.to_string_lossy().into_owned();
            requests.push((
                id,
                if time_freq {
                    BulkRequest::StftRaw {
                        url,
                        channels: data.channels,
                        window_size: el.spectral.fft_size,
                        hop: el.spectral.hop,
                        sample_rate: el.editor.sample_rate,
                    }
                } else {
                    BulkRequest::Raw {
                        url,
                        channels: data.channels,
                        base_bucket: data.base_bucket,
                    }
                },
            ));
        } else if let (Some(bufnum), true) = (data.buffer, data.samples.is_empty()) {
            buffer_refs.push((id, bufnum));
        }
    });
}

/// Fetches one bulk URL and decodes it off the event loop, then hands the
/// result back through the proxy as [`WebEvent::BulkReady`].
pub(super) async fn fetch_bulk(host: HostId, def_id: i32, widget_id: i32, request: BulkRequest) {
    let url = match &request {
        BulkRequest::Cache(url) | BulkRequest::StftCache(url) => url,
        BulkRequest::Raw { url, .. }
        | BulkRequest::StftRaw { url, .. }
        | BulkRequest::Plot { url, .. } => url,
    }
    .clone();
    let bytes = match fetch_bytes(&url).await {
        Ok(bytes) => bytes,
        Err(e) => return log(&format!("bulk fetch {url}: {e}")),
    };
    let data = match request {
        BulkRequest::Cache(_) => {
            let Some(multi) = MultiPyramid::from_bytes(&bytes) else {
                return log(&format!("bulk fetch {url}: malformed peak pyramid"));
            };
            log(&format!(
                "waveform: fetched peak cache {url} ({} samples x {} channel(s), no raw data)",
                multi.frames(),
                multi.num_channels()
            ));
            BulkData::Waveform(WaveformData::with_multi_pyramid(multi))
        }
        BulkRequest::Raw {
            channels,
            base_bucket,
            ..
        } => {
            let flat = decode_f32(&bytes);
            log(&format!(
                "waveform: fetched {} samples x {channels} channel(s) from {url} (pyramids built in wasm)",
                flat.len() / channels.max(1)
            ));
            BulkData::Waveform(WaveformData::from_interleaved(&flat, channels, base_bucket))
        }
        BulkRequest::StftCache(_) => {
            let Some(stft) = Stft::from_bytes(&bytes) else {
                return log(&format!("bulk fetch {url}: malformed STFT cache"));
            };
            log(&format!(
                "spectrogram: fetched STFT cache {url} ({} frames x {} bins)",
                stft.n_frames(),
                stft.n_bins()
            ));
            BulkData::Spectrogram(vec![stft])
        }
        BulkRequest::StftRaw {
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
            BulkData::Spectrogram(stfts)
        }
        BulkRequest::Plot { channels, .. } => {
            let mut flat = decode_f32(&bytes);
            let channels = channels.max(1);
            flat.truncate(flat.len() / channels * channels);
            log(&format!(
                "plot: fetched {} samples x {channels} channel(s) from {url}",
                flat.len() / channels
            ));
            BulkData::Plot(flat.into())
        }
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
    pub(super) fn place_bulk(&mut self, def_id: i32, widget_id: i32, data: BulkData) {
        let Some(slot) = self.canvases.get_mut(&def_id) else {
            return; // the canvas was detached while the fetch was in flight
        };
        let Some(render) = slot.render.as_mut() else {
            slot.pending_bulk.push((widget_id, data));
            return;
        };
        let mut total = None;
        match data {
            BulkData::Waveform(data) => {
                let slot = frame::waveform_slot(data, &render.gpu);
                total = Some(slot.view.total_samples());
                render.waveforms.insert(widget_id, slot);
            }
            BulkData::Spectrogram(stfts) => {
                if let Some(slot) = frame::spectrogram_slot(stfts, &render.gpu, &render.renderers) {
                    total = Some(slot.total_samples());
                    render.spectrograms.insert(widget_id, slot);
                }
            }
            BulkData::Plot(_) => unreachable!("plots are placed in the tree, not the GPU"),
        }
        // The loaded extent joins the widget's navigation group.
        if let Some(total) = total {
            self.host.set_timeline_total(widget_id, total);
        }
    }

    /// Writes a fetched take's pyramid into the signal element that wanted it —
    /// the mesh-drawn counterpart of a plot's samples (a clip body needs no GPU
    /// slot: it is flat geometry, decimated from the take's peak pyramid).
    /// `widget_id` may name the clip rather than the body, which carries no id.
    pub(super) fn set_take_body(&mut self, def_id: i32, widget_id: i32, data: WaveformData) {
        if let Some(root) = self.host.window_def_mut(def_id)
            && let Some(el) = root.find_mut(widget_id).and_then(|w| w.signal_target_mut())
            && let Some(d) = el.source.data_mut()
        {
            d.body = Some(Arc::new(data));
        }
    }

    /// A fetched bulk resource arrived: place a waveform/spectrogram (GPU
    /// slot), write a clip's take or a plot's samples into the host tree, then
    /// repaint.
    pub(super) fn on_bulk_ready(&mut self, def: i32, widget_id: i32, data: BulkData) {
        // A waveform resource wanted by a **mesh-drawn** take (a clip's body)
        // lands in the tree, not the GPU.
        if let BulkData::Waveform(_) = &data
            && self
                .host
                .window_def(def)
                .and_then(|t| t.find(widget_id))
                .and_then(|w| w.signal_target())
                .is_some_and(|el| !el.needs_gpu_slot())
        {
            let BulkData::Waveform(data) = data else {
                unreachable!()
            };
            self.set_take_body(def, widget_id, data);
            self.request_redraw(def);
            return;
        }
        match data {
            BulkData::Waveform(_) | BulkData::Spectrogram(_) => {
                self.place_bulk(def, widget_id, data);
            }
            BulkData::Plot(samples) => {
                if let Some(root) = self.host.window_def_mut(def)
                    && let Some(widget) = root.find_mut(widget_id)
                    && let Some(el) = widget.kind.signal_mut()
                    && let Some(data) = el.source.data_mut()
                {
                    data.samples = samples;
                    // Landed samples feed the spectral presentation: refresh
                    // its cached analysis.
                    widget.kind.refresh_analysis();
                }
            }
        }
        self.request_redraw(def);
    }
}
