//! Window lifecycle: opening a window for a window-rooted GuiDef (GPU bring-up,
//! building the heavy-view slots, mapping bulk resources), and the three ways a
//! window goes away (protocol close, user close, standalone quit).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use clausters_core::osc::{OscMessage, OscType};
use tracing::{info, warn};
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use crate::gpu::Gpu;
use crate::host::bulk::MmapLoader;
use crate::host::canvas::CanvasView;
use crate::host::frame::{self, SpectrogramSlot, WaveformSlot};
use crate::host::paint::Painter;
use crate::host::signal;
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
        let Some(title) = self.host.window_def(id).and_then(|t| match &t.kind {
            WidgetKind::Window { title, .. } => Some(
                title
                    .clone()
                    .unwrap_or_else(|| format!("clausters-gui {id}")),
            ),
            _ => None,
        }) else {
            return; // freed between the effect and now, or not a window
        };
        // What it asks for: its declared size, or its content's where it hugs.
        let Some((width, height)) = self.host.window_size(id) else {
            return;
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
        // A hugging window was sized before it had a scale, and the resolved
        // table snaps its roles to whole pixels — so on a fractional scale the
        // estimate is a pixel or two under what the layout is about to draw.
        // Ask again now that the table is the one the layout will use.
        if let Some((w, h)) = self.host.window_size_px(id) {
            let _ = window.request_inner_size(PhysicalSize::new(w, h));
        }
        let gpu = match pollster::block_on(Gpu::new(window, self.host.msaa)) {
            Ok(gpu) => gpu,
            Err(e) => return warn!("gui_def {id}: cannot start the GPU: {e}"),
        };

        let mut waveforms = HashMap::new();
        let mut spectrograms = HashMap::new();
        let mut buffer_refs = Vec::new();
        let mut canvases = HashMap::new();
        // The window's shared pipelines come first: a spectrogram slot binds its
        // textures against their layout.
        let renderers = Renderers::new(&gpu.device, gpu.target());
        if let Some(tree) = self.host.window_def_mut(id) {
            load_bulk(
                tree,
                None,
                &gpu,
                &renderers,
                &mut waveforms,
                &mut spectrograms,
                &mut buffer_refs,
            );
        }
        if let Some(tree) = self.host.window_def(id) {
            collect_canvases(tree, &gpu, &mut canvases);
        }
        // ...and whatever the elements themselves hold goes up through the same
        // door the tick uses: an element with inline samples fills its slot
        // here, on the window's first frame rather than on its second. The
        // device is new, so the tree is told that whatever it handed a previous
        // one is gone with it.
        let mut extents = Vec::new();
        if let Some(tree) = self.host.window_def_mut(id) {
            frame::slots_dropped(tree);
            frame::fill_slots(
                tree,
                None,
                &gpu,
                &renderers,
                &mut waveforms,
                &mut spectrograms,
                &mut extents,
            );
        }
        self.apply_extents(extents);
        // Register each loaded view's data extent with its navigation group
        // (the group timeline spans the longest member).
        for (wid, slot) in &waveforms {
            self.host
                .set_timeline_total(*wid, slot.view.total_samples());
        }
        for (wid, slot) in &spectrograms {
            self.host.set_timeline_total(*wid, slot.total_samples());
        }
        let painter = Painter::new(&gpu.device, gpu.target());
        let overlay = Painter::new(&gpu.device, gpu.target());

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

/// **Resolves every declared bulk resource** in one walk, and routes what came
/// back the way the declaration says: into the element's **GPU slot** when it
/// claimed one, into the element itself when it did not, and onto the deferred
/// `buffer_refs` list when the resource is a server buffer the client leg has to
/// ask for.
///
/// Nothing here knows what a signal is. Two walks used to: this one matched the
/// arm and re-derived from a presentation whether an element's file became a
/// peak pyramid or a set of analyses, while a second one (`load_element_bulk`)
/// already asked the declaration for everything mesh-drawn. They are one now,
/// and the fork is `Needs::slot` — the same one the page forks on
/// (`host::web::bulk`, which this build does not compile), so a resource that
/// lands in a lane natively
/// lands in the same place in a browser.
///
/// The id a load is keyed by is the **owner's**: a clip's body carries none, so
/// the walk carries the nearest id above it down, which a flat `descendants`
/// pass could not.
fn load_bulk(
    widget: &mut Widget,
    owner: Option<i32>,
    gpu: &Gpu,
    renderers: &Renderers,
    waveforms: &mut HashMap<i32, WaveformSlot>,
    spectrograms: &mut HashMap<i32, SpectrogramSlot>,
    buffer_refs: &mut Vec<(i32, i32)>,
) {
    let owner = widget.id.or(owner);
    let needs = widget.kind.needs();
    if let (Some(id), Some(want)) = (owner, needs.bulk) {
        match want {
            // A server buffer names no local file: the leg fetches it, and the
            // reply lands through the same routing this walk does.
            Bulk::Buffer(bufnum) => buffer_refs.push((id, bufnum)),
            want => {
                if let Some(loaded) = resolve_bulk(&want) {
                    if needs.slot.is_some() {
                        frame::place_in_slot(loaded, id, gpu, renderers, waveforms, spectrograms);
                    } else {
                        widget.kind.take_bulk(loaded);
                    }
                }
            }
        }
    }
    for child in &mut widget.children {
        load_bulk(
            child,
            owner,
            gpu,
            renderers,
            waveforms,
            spectrograms,
            buffer_refs,
        );
    }
}

/// Builds a [`CanvasView`] (compiling the user shader) for every `canvas` in the
/// tree, keyed by widget id.
fn collect_canvases(tree: &Widget, gpu: &Gpu, out: &mut HashMap<i32, CanvasView>) {
    for widget in tree.descendants() {
        if let Some(SlotKind::Shader { source }) = widget.kind.needs().slot
            && let Some(id) = widget.id
        {
            out.insert(id, CanvasView::new(&gpu.device, gpu.target(), &source));
        }
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
        Bulk::StftCache(cache) => match MmapLoader
            .file_bytes(cache)
            .and_then(|bytes| Stft::from_bytes(&bytes))
        {
            Some(stft) => Some(Loaded::Stfts(vec![stft])),
            None => {
                warn!("spectrogram: cannot parse STFT cache {}", cache.display());
                None
            }
        },
        Bulk::Stft {
            path,
            channels,
            window_size,
            hop,
            sample_rate,
        } => MmapLoader.raw_channels(path, *channels).map(|split| {
            let stfts = frame::stft_lanes(split, *window_size, *hop, *sample_rate);
            let frames: usize = stfts.iter().map(|s| s.n_frames()).sum();
            info!(
                "spectrogram: analyzed {} ({} frame(s), no OSC)",
                path.display(),
                frames
            );
            Loaded::Stfts(stfts)
        }),
        // A server buffer is the leg's: it names no local resource, and the
        // walk above never brings one here.
        Bulk::Buffer(_) => None,
    }
}
