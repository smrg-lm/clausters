//! Window lifecycle: opening a window for a window-rooted GuiDef (GPU bring-up,
//! building the heavy-view slots, mapping bulk resources), and the three ways a
//! window goes away (protocol close, user close, standalone quit).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use clausters_core::osc::{OscMessage, OscType};
use tracing::{info, warn};
use winit::dpi::LogicalSize;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use crate::gpu::Gpu;
use crate::host::bulk::MmapLoader;
use crate::host::canvas::CanvasView;
use crate::host::frame::{self, SpectrogramSlot, WaveformSlot};
use crate::host::paint::Painter;
use crate::host::widget::{Widget, WidgetKind};
use crate::host::{BulkLoader, ClientId, GUI_CLOSED};
use crate::spectrogram::Stft;
use crate::view::TimelineView;
use crate::waveform::WaveformData;

use super::app::{App, WindowState};
use super::serverleg::tree_has_node_tree;
use super::{NODETREE_POLL, PLACEHOLDER_ORIGIN};

impl App {
    pub(super) fn open_window(&mut self, event_loop: &ActiveEventLoop, id: i32, origin: ClientId) {
        // Read the window metadata, releasing the host borrow before mutating
        // (drop_window) and before re-borrowing the tree for the waveforms.
        let Some((title, width, height)) = self.host.window_def(id).and_then(|t| match &t.kind {
            WidgetKind::Window {
                title,
                width,
                height,
                ..
            } => Some((
                title
                    .clone()
                    .unwrap_or_else(|| format!("clausters-gui {id}")),
                *width,
                *height,
            )),
            _ => None,
        }) else {
            return; // freed between the effect and now, or not a window
        };

        self.drop_window(id); // rebuild semantics on a re-/gui_def

        let attrs = Window::default_attributes()
            .with_title(title.clone())
            .with_inner_size(LogicalSize::new(width as f64, height as f64));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => return warn!("gui_def {id}: cannot create window: {e}"),
        };
        let winit_id = window.id();
        let gpu = match pollster::block_on(Gpu::new(window)) {
            Ok(gpu) => gpu,
            Err(e) => return warn!("gui_def {id}: cannot start the GPU: {e}"),
        };

        let mut waveforms = HashMap::new();
        let mut spectrograms = HashMap::new();
        let mut buffer_refs = Vec::new();
        let mut canvases = HashMap::new();
        if let Some(tree) = self.host.window_def(id) {
            collect_timelines(
                tree,
                &gpu,
                &mut waveforms,
                &mut spectrograms,
                &mut buffer_refs,
            );
            collect_canvases(tree, &gpu, &mut canvases);
        }
        // Register each loaded view's data extent with its navigation group
        // (the group timeline spans the longest member).
        for (wid, slot) in &waveforms {
            self.host
                .set_timeline_total(*wid, slot.view.total_samples());
        }
        for (wid, slot) in &spectrograms {
            self.host.set_timeline_total(*wid, slot.total_samples());
        }
        let painter = Painter::new(&gpu.device, gpu.config.format);
        let overlay = Painter::new(&gpu.device, gpu.config.format);

        self.by_winit.insert(winit_id, id);
        self.windows.insert(
            id,
            WindowState {
                gpu,
                waveforms,
                spectrograms,
                canvases,
                painter,
                overlay,
                origin,
                cursor: (0.0, 0.0),
                shift: false,
                ctrl: false,
                alt: false,
                gestures: Default::default(),
                scopes: HashMap::new(),
                tap_windows: HashMap::new(),
                spectra: HashMap::new(),
            },
        );
        info!("gui_def {id}: opened window \"{title}\"");
        // Plots and clips that name a local file map it now (the bulk path, no
        // OSC); the samples land in the host tree the renderer reads each frame.
        if let Some(root) = self.host.window_def_mut(id) {
            load_plot_paths(root);
            load_clip_bodies(root);
        }
        if let Some(ws) = self.windows.get(&id) {
            ws.gpu.window.request_redraw();
        }
        // Kick off fetches for any waveform that references a server buffer.
        self.start_buffer_fetches(id, buffer_refs);
        // A node-tree view drives the client leg: register for notifications and
        // query at once, so it shows the tree without waiting for the first poll.
        if self.host.window_def(id).is_some_and(tree_has_node_tree) && self.host.server().is_some()
        {
            self.ensure_notify();
            self.requery_node_trees();
            self.next_query = Instant::now() + NODETREE_POLL;
        }
    }

    pub(super) fn drop_window(&mut self, id: i32) {
        if let Some(ws) = self.windows.remove(&id) {
            self.by_winit.remove(&ws.gpu.window.id());
        }
        // Drop any pending buffer wants this window had, so a finished fetch does
        // not try to fill a window that is gone (or being rebuilt).
        self.fetches.drop_def(id);
    }

    /// User-initiated close: tell the script, then drop the window. A standalone
    /// window has the placeholder origin (UDP port 0) — there is no script to
    /// notify, so the `/gui_closed` is skipped (sending to port 0 fails with
    /// EINVAL).
    fn close_by_user(&mut self, id: i32) {
        if let Some(ws) = self.windows.get(&id)
            && ws.origin != PLACEHOLDER_ORIGIN
        {
            self.send(
                ws.origin,
                OscMessage {
                    addr: GUI_CLOSED.into(),
                    args: vec![OscType::Int(id)],
                },
            );
        }
        self.drop_window(id);
    }

    /// Closes a window on user request, and quits the app once the last window is
    /// gone in standalone mode — so the embedded audio server is dropped (and
    /// `/quit`ed) rather than left running with no window. A script-driven host
    /// stays alive (the script may open another window); only standalone exits.
    pub(super) fn user_close(&mut self, id: i32, event_loop: &ActiveEventLoop) {
        self.close_by_user(id);
        if self.standalone && self.windows.is_empty() {
            event_loop.exit();
        }
    }
}

/// Walks the tree building the timeline views (waveform and spectrogram). A
/// `cache`/`path` resource is loaded **now** from a mapped local file (the
/// bulk path, no OSC); a server-`buffer` reference with no data is deferred as
/// a `(widget_id, bufnum)` entry in `buffer_refs` for the client leg to fetch;
/// inline/blob (and empty) samples build a slot directly.
fn collect_timelines(
    widget: &Widget,
    gpu: &Gpu,
    waveforms: &mut HashMap<i32, WaveformSlot>,
    spectrograms: &mut HashMap<i32, SpectrogramSlot>,
    buffer_refs: &mut Vec<(i32, i32)>,
) {
    match (&widget.kind, widget.id) {
        (
            WidgetKind::Waveform {
                samples,
                base_bucket,
                buffer,
                path,
                cache,
                channels,
                ..
            },
            Some(id),
        ) => {
            if cache.is_some() || path.is_some() {
                // Bulk path: map a local resource (raw samples or a prebuilt
                // cache) through the BulkLoader seam, then build the GPU slot.
                if let Some(data) =
                    MmapLoader.waveform(cache.as_deref(), path.as_deref(), *channels, *base_bucket)
                {
                    waveforms.insert(id, frame::waveform_slot(data, gpu));
                }
            } else if let (Some(bufnum), true) = (buffer, samples.is_empty()) {
                // A server buffer with no inline data: fetch it over the leg.
                buffer_refs.push((id, *bufnum));
            } else {
                waveforms.insert(
                    id,
                    frame::waveform_slot(
                        WaveformData::from_interleaved(samples, *channels, *base_bucket),
                        gpu,
                    ),
                );
            }
        }
        (
            WidgetKind::Spectrogram {
                samples,
                channels,
                buffer,
                path,
                cache,
                window_size,
                hop,
                sample_rate,
                ..
            },
            Some(id),
        ) => {
            if let Some(cache) = cache {
                // A prebuilt (single-channel) STFT cache, parsed directly.
                if let Some(stft) = MmapLoader
                    .file_bytes(cache)
                    .and_then(|bytes| Stft::from_bytes(&bytes))
                {
                    if let Some(slot) = frame::spectrogram_slot(vec![stft], gpu) {
                        spectrograms.insert(id, slot);
                    }
                } else {
                    warn!(
                        "spectrogram {id}: cannot parse STFT cache {}",
                        cache.display()
                    );
                }
            } else if let Some(path) = path {
                if let Some(split) = MmapLoader.raw_channels(path, *channels) {
                    let stfts = frame::stft_lanes(split, *window_size, *hop, *sample_rate);
                    if let Some(slot) = frame::spectrogram_slot(stfts, gpu) {
                        spectrograms.insert(id, slot);
                    }
                }
            } else if let (Some(bufnum), true) = (buffer, samples.is_empty()) {
                buffer_refs.push((id, *bufnum));
            } else if !samples.is_empty() {
                let stfts = frame::stft_lanes(
                    frame::deinterleave(samples, *channels),
                    *window_size,
                    *hop,
                    *sample_rate,
                );
                if let Some(slot) = frame::spectrogram_slot(stfts, gpu) {
                    spectrograms.insert(id, slot);
                }
            }
        }
        (
            WidgetKind::Clip {
                samples, buffer, ..
            },
            Some(id),
        ) => {
            // A clip naming a server buffer with no inline body: fetch it over
            // the leg, exactly like a waveform (the `cache`/`path` bulk bodies
            // are mapped in `load_clip_bodies` when the window opens).
            if let (Some(bufnum), true) = (buffer, samples.is_empty()) {
                buffer_refs.push((id, *bufnum));
            }
        }
        _ => {}
    }
    for child in &widget.children {
        collect_timelines(child, gpu, waveforms, spectrograms, buffer_refs);
    }
}

/// Builds a [`CanvasView`] (compiling the user shader) for every `canvas` in the
/// tree, keyed by widget id.
fn collect_canvases(widget: &Widget, gpu: &Gpu, out: &mut HashMap<i32, CanvasView>) {
    if let WidgetKind::Canvas { shader, .. } = &widget.kind
        && let Some(id) = widget.id
    {
        out.insert(id, CanvasView::new(&gpu.device, gpu.config.format, shader));
    }
    for child in &widget.children {
        collect_canvases(child, gpu, out);
    }
}

/// Maps a `plot`'s local resource into its tree node: a `path` of raw
/// little-endian `f32` mapped read-only, kept interleaved — every channel is
/// drawn (the bulk path, no OSC). Walks children too. Already-loaded (inline)
/// plots and plots without a path are left as they are. Landing samples also
/// refreshes the plot's cached spectral analysis.
fn load_plot_paths(widget: &mut Widget) {
    if let WidgetKind::Plot {
        samples,
        path,
        channels,
        ..
    } = &mut widget.kind
        && samples.is_empty()
        && let Some(p) = path.clone()
        && let Some(loaded) = MmapLoader.plot_samples(&p, *channels)
    {
        *samples = loaded;
        widget.kind.refresh_plot_analysis();
    }
    for child in &mut widget.children {
        load_plot_paths(child);
    }
}

/// Maps the local resource (`cache` or `path`) of every clip that names one,
/// through the same [`BulkLoader`] seam the waveform
/// view uses — so a minutes-long take reaches a lane as a peak pyramid, never as
/// JSON over OSC. The loaded body lands in the host tree (like a plot's
/// samples), no GPU slot: a lane draws flat geometry decimated from the pyramid.
#[allow(clippy::type_complexity)]
fn load_clip_bodies(widget: &mut Widget) {
    if let WidgetKind::Clip {
        body,
        path,
        cache,
        channels,
        base_bucket,
        ..
    } = &mut widget.kind
        && body.is_none()
        && (path.is_some() || cache.is_some())
        && let Some(data) =
            MmapLoader.waveform(cache.as_deref(), path.as_deref(), *channels, *base_bucket)
    {
        *body = Some(Arc::new(data));
    }
    for child in &mut widget.children {
        load_clip_bodies(child);
    }
}
