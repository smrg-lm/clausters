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
use crate::host::signal::{self, Presentation};
use crate::host::widget::element::{Bulk, Loaded, SlotKind};
use crate::host::widget::{Widget, WidgetKind};
use crate::host::{BulkLoader, ClientId, GUI_CLOSED};
use crate::spectrogram::Stft;
use crate::view::Renderers;
use crate::view::TimelineView;

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
        // The window's UI scale, the one platform reading the host cannot make
        // itself: the shell writes it, the core resolves its size table once and
        // the wire's logical lengths land on this display's pixels.
        let ui_scale = window.scale_factor();
        self.host.set_ui_scale(id, ui_scale as f32);
        let gpu = match pollster::block_on(Gpu::new(window)) {
            Ok(gpu) => gpu,
            Err(e) => return warn!("gui_def {id}: cannot start the GPU: {e}"),
        };

        let mut waveforms = HashMap::new();
        let mut spectrograms = HashMap::new();
        let mut buffer_refs = Vec::new();
        let mut canvases = HashMap::new();
        // The window's shared pipelines come first: a spectrogram slot binds its
        // textures against their layout.
        let renderers = Renderers::new(&gpu.device, gpu.config.format);
        if let Some(tree) = self.host.window_def(id) {
            collect_timelines(
                tree,
                None,
                &gpu,
                &renderers,
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
                renderers,
                painter,
                overlay,
                origin,
                cursor: (0.0, 0.0),
                shift: false,
                ctrl: false,
                alt: false,
                gestures: Default::default(),
                histories: HashMap::new(),
            },
        );
        // The scale is in the line because it is the one thing about a window
        // nobody can read off a screenshot: a desktop that ignores what was
        // asked of it (an X11-only override under Wayland, say) looks exactly
        // like a host that ignored it.
        info!("gui_def {id}: opened window \"{title}\" at scale {ui_scale}");
        // Every mesh-drawn element that names a local file maps it now (the bulk
        // path, no OSC); the data lands in the host tree the renderer reads each
        // frame — a sequence as samples, a take as its peak pyramid.
        if let Some(root) = self.host.window_def_mut(id) {
            load_element_bulk(root);
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
    /// `/server_quit`ed) rather than left running with no window. A script-driven host
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
    owner: Option<i32>,
    gpu: &Gpu,
    renderers: &Renderers,
    waveforms: &mut HashMap<i32, WaveformSlot>,
    spectrograms: &mut HashMap<i32, SpectrogramSlot>,
    buffer_refs: &mut Vec<(i32, i32)>,
) {
    frame::visit_elements(widget, owner, &mut |owner, el| {
        let (Some(id), Some(data)) = (owner, el.source.data()) else {
            return;
        };
        // A **mesh-drawn** bulk source naming a server buffer and holding
        // nothing yet: fetch it over the leg, exactly as a heavy view does.
        // This is a clip's take — it owns no GPU slot, and its id is the
        // clip's, since a body carries none of its own.
        if !el.needs_gpu_slot() {
            if data.bulk
                && data.is_empty()
                && let Some(bufnum) = data.buffer
            {
                buffer_refs.push((id, bufnum));
            }
            return;
        }
        // A navigable heavy view needs a slot; so does a **spectral clip
        // body**, whose picture is a texture and cannot be drawn into the mesh
        // the way a take's trace is.
        let (cache, path) = (data.cache.as_deref(), data.path.as_deref());
        if el.presentation == Presentation::Signal {
            if cache.is_some() || path.is_some() {
                // Bulk path: map a local resource (raw samples or a prebuilt
                // cache) through the BulkLoader seam, then build the GPU slot.
                if let Some(loaded) =
                    MmapLoader.waveform(cache, path, data.channels, data.base_bucket)
                {
                    waveforms.insert(id, frame::waveform_slot(loaded, gpu));
                }
                return;
            }
        } else if let Some(cache) = cache {
            // A prebuilt (single-channel) STFT cache, parsed directly.
            match MmapLoader
                .file_bytes(cache)
                .and_then(|bytes| Stft::from_bytes(&bytes))
            {
                Some(stft) => {
                    if let Some(slot) = frame::spectrogram_slot(vec![stft], gpu, renderers) {
                        spectrograms.insert(id, slot);
                    }
                }
                None => warn!(
                    "spectrogram {id}: cannot parse STFT cache {}",
                    cache.display()
                ),
            }
            return;
        } else if let Some(path) = path {
            if let Some(split) = MmapLoader.raw_channels(path, data.channels) {
                let stfts = frame::stft_lanes(
                    split,
                    el.spectral.fft_size,
                    el.spectral.hop,
                    el.editor.sample_rate,
                );
                let frames: usize = stfts.iter().map(|s| s.n_frames()).sum();
                if let Some(slot) = frame::spectrogram_slot(stfts, gpu, renderers) {
                    info!(
                        "spectrogram {id}: analyzed {} from {} ({} frame(s), no OSC)",
                        path.display(),
                        if el.caps.navigable {
                            "a view"
                        } else {
                            "a clip"
                        },
                        frames
                    );
                    spectrograms.insert(id, slot);
                }
            }
            return;
        }
        // No local resource named: a server buffer with no inline data is
        // deferred to the leg, anything else builds its slot from what the
        // host already holds.
        match data.buffer.filter(|_| data.samples.is_empty()) {
            Some(bufnum) => buffer_refs.push((id, bufnum)),
            None => frame::inline_slot(id, el, data, gpu, renderers, waveforms, spectrograms),
        }
    });
}

/// Builds a [`CanvasView`] (compiling the user shader) for every `canvas` in the
/// tree, keyed by widget id.
fn collect_canvases(tree: &Widget, gpu: &Gpu, out: &mut HashMap<i32, CanvasView>) {
    for widget in tree.descendants() {
        if let Some(SlotKind::Shader { source }) = widget.kind.needs().slot
            && let Some(id) = widget.id
        {
            out.insert(id, CanvasView::new(&gpu.device, gpu.config.format, &source));
        }
    }
}

/// Resolves every element's **declared** bulk resource into it, through the
/// [`BulkLoader`] seam — so a minutes-long take reaches a lane as a peak
/// pyramid and never as JSON over OSC. Walks children too, which is how a
/// clip's take is reached.
///
/// Nothing here knows what a signal is. An element says which resource it wants
/// and in which form (`Needs::bulk`), the loader maps that form, and the
/// element takes the answer home (`Element::bulk`). An element that claimed a
/// **GPU slot** is skipped: its data is fed to a pipeline, above.
fn load_element_bulk(widget: &mut Widget) {
    let needs = widget.kind.needs();
    if let (Some(want), None) = (needs.bulk, needs.slot)
        && let Some(loaded) = resolve_bulk(&want)
    {
        widget.kind.take_bulk(loaded);
    }
    for child in &mut widget.children {
        load_element_bulk(child);
    }
}

/// Maps one declared resource with the native loader. A `Buffer` resolves to
/// nothing here — it is the client leg's, and the reply lands the same way.
fn resolve_bulk(want: &Bulk) -> Option<Loaded> {
    match want {
        Bulk::PeakCache(cache) => MmapLoader
            .waveform(Some(cache), None, 1, signal::DEFAULT_BASE_BUCKET)
            .map(Loaded::Peaks),
        Bulk::Peaks {
            path,
            channels,
            base_bucket,
        } => MmapLoader
            .waveform(None, Some(path), *channels, *base_bucket)
            .map(Loaded::Peaks),
        Bulk::Samples { path, channels } => MmapLoader
            .plot_samples(path, *channels)
            .map(Loaded::Samples),
        // The spectral forms belong to a GPU slot, which is fed above; a
        // mesh-drawn element never asks for them.
        Bulk::StftCache(_) | Bulk::Stft { .. } | Bulk::Buffer(_) => None,
    }
}
