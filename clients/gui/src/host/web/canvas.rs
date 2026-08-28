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
    /// **Which mouse buttons the browser says are down**, as of the last
    /// pointer event on this canvas — the bitmask of `PointerEvent.buttons`.
    ///
    /// A desktop window cannot lose a release: the OS delivers the button-up to
    /// whoever captured the pointer. A page can — the button comes up outside
    /// the browser window, over another application, after an alt-tab — and
    /// winit synthesizes a button event only from a move that *reports* a
    /// change (`PointerEvent.button != -1`), which such a move does not. So the
    /// gesture machine would still be holding whatever was in hand, and the
    /// next move back over the canvas would look exactly like a drag step: the
    /// thing in hand teleports to wherever the pointer came back in, and the
    /// edit it amounts to is never sent because no release ever happens.
    ///
    /// The shell reads the browser's own answer and ends the drag itself. This
    /// is the browser front's job and not the host's: the host is one
    /// implementation and must be handed the same press → drag → release either
    /// side, which is what this restores.
    pub(super) buttons: Rc<Cell<u16>>,
    /// Kept alive for as long as the canvas is: dropping it removes the
    /// listener that fills it.
    pub(super) pointer_listener: Option<PointerListener>,
    /// The finger currently driving this canvas, if any.
    ///
    /// The gesture machine is single-pointer — one press, one drag, one release
    /// — so the **first** touch owns the gesture and the rest are ignored until
    /// it lifts. A second finger landing mid-drag would otherwise teleport the
    /// value being dragged.
    pub(super) touch: Option<u64>,
    /// This canvas' gesture state — the shared machine both fronts drive.
    pub(super) gestures: Gestures,
    /// **Which modifier keys are down**, as the browser reported them on the
    /// last pointer or wheel event over this canvas: shift, ctrl, alt, in that
    /// bit order. Snapshotted into each [`GestureCtx`] so Shift-pan /
    /// Ctrl-edit / Alt-select work as on the desktop.
    ///
    /// Read from the **event** rather than tracked from winit's
    /// `ModifiersChanged`, which a page gets only while the canvas has DOM
    /// focus (`window_target.rs`: `has_focus.get() && …`). A reader who has not
    /// clicked the canvas yet holds Shift and pans nothing; one who releases it
    /// after clicking away leaves the front believing it is still down, and a
    /// plain drag pans. Every pointer event carries `shiftKey`/`ctrlKey`/
    /// `altKey` whatever has focus, which is the same fact without the gap —
    /// the reason [`CanvasSlot::buttons`] is read the same way.
    pub(super) mods: Rc<Cell<u8>>,
    /// The retained history per watched **bus** — the browser half of
    /// `retention`, filled from the `/bus_tapStream.reply` store exactly as the
    /// native tick fills it from the segment. What each *view* makes of a
    /// history is the view's own, and lives in the element.
    pub(super) histories: HashMap<i32, live::BusHistory>,
    /// Fetched waveforms/spectrograms that arrived before the GPU was ready,
    /// placed on `GpuReady` (plots need no GPU and are placed immediately).
    pub(super) pending_bulk: Vec<(i32, Loaded)>,
}

/// The window-level pointer listener a [`CanvasSlot`] keeps, removed when the
/// canvas goes: a detached canvas whose listener stayed would keep reading a
/// rectangle nothing draws into.
pub(super) struct PointerListener {
    window: web_sys::Window,
    closure: Closure<dyn FnMut(web_sys::MouseEvent)>,
}

/// The events a gesture is made of, watched together — the three pointer ones
/// and the wheel, which carries the modifiers a zoom is qualified by.
const POINTER_EVENTS: [&str; 4] = ["pointerdown", "pointermove", "pointerup", "wheel"];

impl Drop for PointerListener {
    fn drop(&mut self) {
        for name in POINTER_EVENTS {
            let _ = self.window.remove_event_listener_with_callback_and_bool(
                name,
                self.closure.as_ref().unchecked_ref(),
                true,
            );
        }
    }
}

impl CanvasSlot {
    /// Shift, ctrl, alt as of the last pointer or wheel event (see
    /// [`CanvasSlot::mods`]).
    pub(super) fn modifiers(&self) -> (bool, bool, bool) {
        let m = self.mods.get();
        (m & 1 != 0, m & 2 != 0, m & 4 != 0)
    }

    pub(super) fn new(window: Arc<Window>) -> Self {
        Self {
            window,
            render: None,
            pending_size: None,
            visible: true,
            cursor: (0.0, 0.0),
            buttons: Rc::new(Cell::new(0)),
            mods: Rc::new(Cell::new(0)),
            pointer_listener: None,
            touch: None,
            gestures: Gestures::default(),
            histories: HashMap::new(),
            pending_bulk: Vec::new(),
        }
    }

    /// Follows the browser's own **button state** and **modifier keys** on this
    /// canvas, so a release the page never saw is caught on the next move (see
    /// [`CanvasSlot::buttons`]) and a Shift-drag means Shift whether or not the
    /// canvas has been clicked yet (see [`CanvasSlot::mods`]).
    ///
    /// On `window` in the capture phase, so the mask is already stored by the
    /// time winit's own handler turns the event into a `CursorMoved`. Anything
    /// whose target is not this canvas is another canvas' (or the page's) and
    /// is left alone.
    fn watch_buttons(&mut self) {
        let Some(canvas) = self.window.canvas() else {
            return;
        };
        let Some(window) = web_sys::window() else {
            return;
        };
        let buttons = self.buttons.clone();
        let mods = self.mods.clone();
        let target = canvas.clone();
        let closure =
            Closure::<dyn FnMut(web_sys::MouseEvent)>::new(move |event: web_sys::MouseEvent| {
                // **A canvas inside a component is in a shadow root**, and an
                // event seen on `window` has already been *retargeted* to the
                // host element -- `target` is the `<clausters-...>` tag, never
                // the canvas, so the mask would stay 0 for every component on
                // the page and the first move of any drag would look like a
                // release the page never saw (see `CanvasSlot::buttons`): a
                // held key let go the moment the pointer moved, a ruler that
                // could not be dragged at all. The composed path is the one
                // view that still names the canvas itself.
                let canvas: &JsValue = target.as_ref();
                let mine = event.target().as_ref() == Some(target.as_ref())
                    || event.composed_path().iter().any(|node| &node == canvas);
                if mine {
                    // A wheel event is a `MouseEvent` too but carries no button
                    // mask worth reading, so only the pointer ones set it.
                    if let Some(pointer) = event.dyn_ref::<web_sys::PointerEvent>() {
                        buttons.set(pointer.buttons());
                    }
                    mods.set(
                        u8::from(event.shift_key())
                            | u8::from(event.ctrl_key()) << 1
                            | u8::from(event.alt_key()) << 2,
                    );
                }
            });
        let mut attached = false;
        for name in POINTER_EVENTS {
            attached |= window
                .add_event_listener_with_callback_and_bool(
                    name,
                    closure.as_ref().unchecked_ref(),
                    true,
                )
                .is_ok();
        }
        if attached {
            self.pointer_listener = Some(PointerListener { window, closure });
        }
    }

    /// Forgets everything derived from a def's tree, keeping the canvas itself
    /// — the rebuild semantics of a re-`/gui_def` and of a `/gui_free`.
    pub(super) fn clear_def_state(&mut self) {
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
        let mut slot = CanvasSlot::new(window.clone());
        slot.watch_buttons();
        self.canvases.insert(def_id, slot);
        log(&format!(
            "def {def_id}: canvas attached; requesting GPU adapter"
        ));
        let (proxy, host) = (web_proxy(), self.id);
        let samples = self.host.msaa;
        wasm_bindgen_futures::spawn_local(async move {
            match Gpu::new(window, samples).await {
                Ok(gpu) => {
                    if let Some(proxy) = proxy {
                        let _ = proxy.send_event(HostEvent::To(
                            host,
                            WebEvent::GpuReady {
                                def_id,
                                gpu: Box::new(gpu),
                            },
                        ));
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

    /// (Re)builds one canvas' GPU resources from what the tree already holds:
    /// every element that claimed a slot fills it here (`path`/`cache`/`buffer`
    /// references load async through
    /// [`fetch_bulk`](super::bulk::fetch_bulk) and the fetch machine).
    ///
    /// Called with a **fresh device** — a canvas attached, a GPU that just came
    /// up — so the slots start empty and the tree is told that whatever it had
    /// handed over is gone.
    pub(super) fn build_resources(&mut self, def: i32) {
        let Some(slot) = self.canvases.get_mut(&def) else {
            return;
        };
        let Some(render) = slot.render.as_mut() else {
            return;
        };
        let Some(tree) = self.host.window_def_mut(def) else {
            return;
        };
        render.waveforms.clear();
        render.spectrograms.clear();
        frame::slots_dropped(tree);
        let mut extents = Vec::new();
        frame::fill_slots(
            tree,
            None,
            &render.gpu,
            &render.renderers,
            &mut render.waveforms,
            &mut render.spectrograms,
            &mut extents,
        );
        self.apply_extents(extents);
    }

    /// Renders one canvas' def through the shared frame path. The live inputs
    /// come from the streamed buses (meters/canvases read them in `render`, the
    /// scopes their tick-fed histories); the node tree stays empty until a
    /// browser node-tree path exists.
    pub(super) fn draw(&mut self, def: i32) {
        // Whatever an element has for its slot reaches the card before the
        // frame that draws it — a canvas with nothing live in it never ticks.
        if let (Some(slot), Some(tree)) =
            (self.canvases.get_mut(&def), self.host.window_def_mut(def))
        {
            let extents = refresh_slots(slot, tree);
            self.apply_extents(extents);
        }
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
            // The page draws what the desktop draws: a held clip keeps its
            // grip and nothing else lights up, whichever front is driving.
            grab: slot.gestures.grab(),
        };
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
            tree,
            &inputs,
            theme,
        );
        // **What this frame could not draw.** A view zoomed finer than its
        // summary left the span it was asked for on its slot; a page cannot
        // map the samples, so it reads exactly that span back — which is what
        // makes the picture resolve to the sample here as it does natively.
        self.fetch_wanted_spans(def);
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
