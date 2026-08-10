//! One host instance's **canvases**: the per-`window`-def surface, its GPU
//! resources and the draw.
//!
//! The browser counterpart of the native front's window lifecycle
//! (`gui::windows`, which this build does not compile): a page hands a `<canvas>` per
//! `window`-rooted def to [`GuiBridge::attach`](super::bridge::GuiBridge), and
//! everything a def needs to be painted -- the wgpu surface, the heavy views'
//! slots, the two painters, the gesture state -- hangs off the [`CanvasSlot`]
//! it opens. The draw itself is [`super::super::frame::render`], the same one
//! the desktop calls, which is what makes the two pixel-faithful.

use super::*;
use crate::host::world::World;

/// The per-canvas GPU resources.
pub(super) struct WindowRender {
    pub(super) gpu: Gpu,
    /// The heavy views' shared pipelines, one set per canvas — the browser twin
    /// of the native front's per-window set.
    pub(super) renderers: Renderers,
    pub(super) painter: Painter,
    /// The editor-chrome overlay pass (selection, playhead, rulers, readout).
    pub(super) overlay: Painter,
    pub(super) waveforms: HashMap<i32, WaveformSlot>,
    pub(super) spectrograms: HashMap<i32, SpectrogramSlot>,
}

/// One canvas: a `window`-rooted GuiDef's drawing surface and everything that
/// follows it. The browser twin of the native front's `WindowState` — the
/// desktop already keeps one of these per window-rooted def, and a document
/// holds N canvases for the same reason a desktop holds N windows.
///
/// The host learns nothing about HTML from it: the page says *this def draws
/// into this canvas, at this size, and right now it is (not) visible*.
pub(super) struct CanvasSlot {
    pub(super) window: Arc<Window>,
    /// The GPU resources, once the async device resolved.
    pub(super) render: Option<WindowRender>,
    /// A size that arrived before the GPU was ready (so `render` was `None` and
    /// it could not be applied yet); replayed on `GpuReady` so the surface is
    /// configured to the real size for the first frame, not a stale 1x1.
    pub(super) pending_size: Option<(u32, u32)>,
    /// Whether the canvas is in the viewport. A hidden one is skipped on the
    /// tick and its buses leave the subscription: a document can hold fifty
    /// canvases with three in view, and the browser's own compositing skip does
    /// not stop *us* from computing a frame or the server from streaming for it.
    pub(super) visible: bool,
    pub(super) cursor: (f64, f64),
    /// The finger currently driving this canvas, if any.
    ///
    /// The gesture machine is single-pointer — one press, one drag, one release
    /// — so the **first** touch owns the gesture and the rest are ignored until
    /// it lifts. A second finger landing mid-drag would otherwise teleport the
    /// value being dragged.
    pub(super) touch: Option<u64>,
    /// This canvas' gesture state — the shared machine both fronts drive.
    pub(super) gestures: Gestures,
    /// Modifier keys (winit `ModifiersChanged`), snapshotted into each
    /// [`GestureCtx`] so Shift-pan/Ctrl-edit/Alt-select work as on the desktop.
    pub(super) shift: bool,
    pub(super) ctrl: bool,
    pub(super) alt: bool,
    /// Recent control-bus samples per `scope` widget id (oldest .. newest),
    /// advanced on [`WebEvent::Tick`] exactly as the native tick does.
    pub(super) scopes: HashMap<i32, VecDeque<f32>>,
    /// Triggered display window per audio-rate scope widget id, refreshed on
    /// the tick (`live::update_tap_windows`). Also holds each phasescope's
    /// interleaved L/R window (ids do not collide).
    pub(super) tap_windows: HashMap<i32, live::TapWindow>,
    /// Persistent FFT analysis state per `spectrum` widget id, advanced on the
    /// tick (`live::update_spectra`), exactly as the native front does.
    pub(super) spectra: HashMap<i32, Vec<SpectrumState>>,
    /// The retained history per watched **bus**, and the rolling
    /// time-frequency analysis per retaining **widget** — the browser half of
    /// `retention`, filled from the `/bus_tapStream.reply` store exactly as the
    /// native tick fills them from the segment.
    pub(super) histories: HashMap<i32, live::BusHistory>,
    pub(super) rolls: HashMap<i32, crate::host::waterfall::Waterfall>,
    /// Fetched waveforms/spectrograms that arrived before the GPU was ready,
    /// placed on `GpuReady` (plots need no GPU and are placed immediately).
    pub(super) pending_bulk: Vec<(i32, BulkData)>,
}

impl CanvasSlot {
    pub(super) fn new(window: Arc<Window>) -> Self {
        Self {
            window,
            render: None,
            pending_size: None,
            visible: true,
            cursor: (0.0, 0.0),
            touch: None,
            gestures: Gestures::default(),
            shift: false,
            ctrl: false,
            alt: false,
            scopes: HashMap::new(),
            tap_windows: HashMap::new(),
            spectra: HashMap::new(),
            histories: HashMap::new(),
            rolls: HashMap::new(),
            pending_bulk: Vec::new(),
        }
    }

    /// Forgets everything derived from a def's tree, keeping the canvas itself
    /// — the rebuild semantics of a re-`/gui_def` and of a `/gui_free`.
    pub(super) fn clear_def_state(&mut self) {
        self.scopes.clear();
        self.tap_windows.clear();
        self.spectra.clear();
        self.pending_bulk.clear();
        if let Some(render) = self.render.as_mut() {
            render.waveforms.clear();
            render.spectrograms.clear();
        }
    }

    pub(super) fn fb(&self) -> (u32, u32) {
        self.render
            .as_ref()
            .map(|r| (r.gpu.config.width.max(1), r.gpu.config.height.max(1)))
            .unwrap_or((1, 1))
    }

    pub(super) fn request_redraw(&self) {
        self.window.request_redraw();
    }
}

impl WebApp {
    /// Gives `def_id` a canvas and starts its GPU bring-up.
    ///
    /// `canvas` is the element the component created — the correct ownership,
    /// and the only way N of them can exist. `None` keeps the older posture, a
    /// canvas winit appends to `<body>`, which is what a page that feeds a
    /// `/gui_def` without attaching anything gets.
    pub(super) fn attach(
        &mut self,
        event_loop: &ActiveEventLoop,
        def_id: i32,
        canvas: Option<web_sys::HtmlCanvasElement>,
    ) {
        if !self.resumed {
            // A window cannot be created before the loop resumes; `resumed`
            // drains this.
            self.pending_attach.push((def_id, canvas));
            return;
        }
        if let Some(old) = self.canvases.remove(&def_id) {
            self.by_winit.remove(&old.window.id());
        }
        let appending = canvas.is_none();
        let attrs = Window::default_attributes()
            .with_title(format!("clausters-gui {def_id}"))
            .with_inner_size(LogicalSize::new(CANVAS_SIZE.0 as f64, CANVAS_SIZE.1 as f64))
            // Not focused on creation: winit focuses a new canvas, and a
            // browser scrolls a freshly focused element into view — so in a
            // document with several components the last one mounted would yank
            // the reader down to it. A click focuses it, which is when keyboard
            // input is wanted anyway.
            .with_active(false)
            .with_canvas(canvas)
            .with_append(appending);
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => return log(&format!("def {def_id}: cannot open a canvas: {e}")),
        };
        self.by_winit.insert(window.id(), def_id);
        self.canvases
            .insert(def_id, CanvasSlot::new(window.clone()));
        log(&format!(
            "def {def_id}: canvas attached; requesting GPU adapter"
        ));
        let (proxy, host) = (web_proxy(), self.id);
        wasm_bindgen_futures::spawn_local(async move {
            match Gpu::new(window).await {
                Ok(gpu) => {
                    if let Some(proxy) = proxy {
                        let _ = proxy
                            .send_event(HostEvent::To(host, WebEvent::GpuReady { def_id, gpu }));
                    }
                }
                Err(e) => {
                    // No GPU adapter at all (neither WebGPU nor WebGL2): surface
                    // a clear, actionable message instead of aborting; the canvas
                    // stays blank but the page survives.
                    log(&e);
                    set_status(&e);
                }
            }
        });
    }

    /// Drops a def's canvas: the wgpu surface and every derived resource go.
    /// The `<canvas>` element itself belongs to the page, which removes it.
    pub(super) fn detach(&mut self, def_id: i32) {
        if let Some(slot) = self.canvases.remove(&def_id) {
            self.by_winit.remove(&slot.window.id());
        }
        self.pending_attach.retain(|(id, _)| *id != def_id);
        self.fetches.drop_def(def_id);
        self.on_tree_changed();
    }

    /// (Re)builds one canvas' GPU resources: the inline-data waveform/
    /// spectrogram views (`path`/`cache`/`buffer` references load async through
    /// [`fetch_bulk`](super::bulk::fetch_bulk) and the fetch machine).
    pub(super) fn build_resources(&mut self, def: i32) {
        let Some(slot) = self.canvases.get(&def) else {
            return;
        };
        let Some(render) = slot.render.as_ref() else {
            return;
        };
        let Some(tree) = self.host.window_def(def) else {
            return;
        };
        let mut waveforms = HashMap::new();
        let mut spectrograms = HashMap::new();
        build_inline_timelines(
            tree,
            None,
            &render.gpu,
            &render.renderers,
            &mut waveforms,
            &mut spectrograms,
        );
        // Each inline view's extent joins its navigation group.
        let mut totals: Vec<(i32, usize)> = Vec::new();
        totals.extend(
            waveforms
                .iter()
                .map(|(id, s)| (*id, s.view.total_samples())),
        );
        totals.extend(spectrograms.iter().map(|(id, s)| (*id, s.total_samples())));
        if let Some(render) = self.canvases.get_mut(&def).and_then(|s| s.render.as_mut()) {
            render.waveforms = waveforms;
            render.spectrograms = spectrograms;
        }
        for (id, total) in totals {
            self.host.set_timeline_total(id, total);
        }
    }

    /// Renders one canvas' def through the shared frame path. The live inputs
    /// come from the streamed buses (meters/canvases read them in `render`, the
    /// scopes their tick-fed histories); the node tree stays empty until a
    /// browser node-tree path exists.
    pub(super) fn draw(&mut self, def: i32) {
        let server_attached = self.host.server().is_some();
        let focused = self
            .host
            .focused()
            .filter(|(d, _)| *d == def)
            .map(|(_, id)| id);
        let timelines = self.host.timelines();
        let theme = &self.host.theme;
        let Some(tree) = self.host.window_def(def) else {
            return;
        };
        let Some(slot) = self.canvases.get_mut(&def) else {
            return;
        };
        let inputs = frame::FrameInputs {
            metrics: self.host.metrics_for(def),
            world: World {
                bus: Some(self.buses.as_ref() as &dyn BusSource),
                server_attached,
                sample_rate: self.server_rate,
                sample_clock: self.server_clock,
                cursor: Some(slot.cursor),
                timelines,
                // The node tree stays empty until a browser node-tree path
                // exists.
                ..Default::default()
            },
            focused,
            // A rewiring drag in flight draws its wire to the pointer.
            wiring: slot
                .gestures
                .wiring()
                .map(|(id, port)| (id, port, (slot.cursor.0 as f32, slot.cursor.1 as f32))),
            marquee: slot.gestures.marquee(),
            ..Default::default()
        };
        let scopes = &slot.scopes;
        let tap_windows = &slot.tap_windows;
        let spectra = &slot.spectra;
        let Some(render) = slot.render.as_mut() else {
            return;
        };
        let mut canvases = HashMap::new();
        frame::render(
            &mut render.gpu,
            &mut render.renderers,
            &mut render.painter,
            &mut render.overlay,
            &mut render.waveforms,
            &mut render.spectrograms,
            &mut canvases,
            scopes,
            tap_windows,
            spectra,
            tree,
            &inputs,
            theme,
        );
    }

    /// Schedules a repaint of one canvas through winit's redraw request
    /// (drawing happens in `RedrawRequested`, the idiomatic path on the
    /// browser's animation frame).
    pub(super) fn request_redraw(&self, def: i32) {
        if let Some(slot) = self.canvases.get(&def) {
            slot.request_redraw();
        }
    }

    /// **Hands the keyboard back to the document**: blurs this def's canvas, so
    /// the browser's own sequential navigation carries on past the mounted
    /// GuiDef instead of the canvas holding every key forever.
    ///
    /// It is the browser half of [`GestureEffect::FocusOut`] and the reason
    /// that effect exists. A canvas is focusable (winit gives it a `tabindex`)
    /// and winit prevents the default on the keys it sees, so Tab inside a
    /// mounted def would otherwise never reach the page: blurring is what makes
    /// the ring an *entrance and an exit* rather than a trap. The page decides
    /// nothing here — it is the host that knows the ring ran out.
    pub(super) fn blur(&self, def: i32) {
        use winit::platform::web::WindowExtWebSys;
        let Some(canvas) = self.canvases.get(&def).and_then(|s| s.window.canvas()) else {
            return;
        };
        if let Err(e) = canvas.blur() {
            log(&format!("def {def}: cannot release the keyboard: {e:?}"));
        }
    }
}
