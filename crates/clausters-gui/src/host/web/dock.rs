//! **The dock, in a page**: one canvas whose surface shows every window
//! the host opens as a pane of it ([`crate::host::dock`]), once the page
//! handed one over ([`GuiBridge::dock`](super::bridge::GuiBridge)).
//!
//! The browser twin of the native front's `gui::dock`, which this build
//! does not compile: a pane is a [`CanvasSlot`] in every way but its canvas --
//! it draws with the dock's canvas window, into a texture of its own on
//! the dock's device -- and [`WebApp::on_def_event`] is what it hears.
//! What this adds is the canvas, its routing (to a pane, or to the dock's
//! own chrome), the frame that copies every shown pane into its surface with
//! the chrome over it, and the dock's door on the wire (`/gui_dock`).

use std::collections::HashSet;

use clausters_gui_draw::draw::Draw;
use clausters_gui_draw::paint::Mesh;
use serde_json::Value;

use super::canvas::{PointerListener, watch_buttons};
use super::capture::CaptureOf;
use super::*;
use crate::host::dock::call::{self, Call};
use crate::host::dock::{Composer, Dock, divider_color};
use crate::host::{PointerKind, PointerMods};

/// The dock's canvas, its surface once the GPU is up, and its panes.
pub(super) struct WebShell {
    window: Arc<Window>,
    /// The surface's GPU and the copy into it, once the device resolved.
    render: Option<(Gpu, Composer)>,
    layout: Dock,
    /// The page's density, which every pane is laid out at.
    scale: f32,
    /// What the browser says of the buttons and the modifiers over the
    /// canvas, shared with every pane's slot (`CanvasSlot::buttons`).
    buttons: Rc<Cell<u16>>,
    mods: Rc<Cell<u8>>,
    _listener: Option<PointerListener>,
    /// A size the page reported before the GPU came up.
    pending_size: Option<(u32, u32)>,
    /// The panes behind another's tab, told they are not seen.
    hidden: HashSet<i32>,
}

impl WebApp {
    /// Whether `id` is the dock's canvas window.
    pub(super) fn is_dock_window(&self, id: WindowId) -> bool {
        self.dock.as_ref().is_some_and(|s| s.window.id() == id)
    }

    /// Whether canvas slot `def` is a pane of the dock.
    pub(super) fn is_pane(&self, def: i32) -> bool {
        self.dock.as_ref().is_some_and(|s| s.layout.holds(def))
    }

    /// **The page hands the dock its canvas**: every window opened from
    /// now on is a pane of it, and so is every window already open.
    pub(super) fn open_dock(
        &mut self,
        event_loop: &ActiveEventLoop,
        canvas: web_sys::HtmlCanvasElement,
    ) {
        if !self.resumed {
            self.pending_dock = Some(canvas);
            return;
        }
        if self.dock.is_some() {
            return log("the dock already has its canvas");
        }
        let attrs = Window::default_attributes()
            .with_title("clausters dock")
            .with_active(false)
            .with_canvas(Some(canvas))
            .with_append(false);
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => return log(&format!("dock: cannot open its canvas: {e}")),
        };
        let buttons = Rc::new(Cell::new(0));
        let mods = Rc::new(Cell::new(0));
        let listener = watch_buttons(&window, &buttons, &mods);
        let scale = window.scale_factor() as f32;
        let metrics = self.host.metrics.resolved(scale);
        self.dock = Some(WebShell {
            window: window.clone(),
            render: None,
            layout: Dock::new((1, 1), metrics),
            scale,
            buttons,
            mods,
            _listener: listener,
            pending_size: None,
            hidden: HashSet::new(),
        });
        self.host.dock = true;
        // the windows already open move into it
        let open: Vec<i32> = self.canvases.keys().copied().collect();
        for def in open {
            self.attach(event_loop, def, None);
        }
        log("dock: canvas attached; requesting GPU adapter");
        let (proxy, host) = (web_proxy(), self.id);
        let samples = self.host.msaa;
        wasm_bindgen_futures::spawn_local(async move {
            match Gpu::new(window, samples).await {
                Ok(gpu) => {
                    if let Some(proxy) = proxy {
                        let _ = proxy
                            .send_event(HostEvent::To(host, WebEvent::DockGpuReady(Box::new(gpu))));
                    }
                }
                Err(e) => {
                    log(&e);
                    set_status(&e);
                }
            }
        });
    }

    /// **Canvas slot `def` as a pane**: the dock's canvas window, the
    /// dock's buttons and modifiers, a texture of its own once the
    /// device is up. The panes already there make room for it.
    pub(super) fn attach_pane(&mut self, def_id: i32) {
        let Some(shell) = self.dock.as_mut() else {
            return;
        };
        if let Some(old) = self.canvases.remove(&def_id) {
            // a canvas of its own it had goes; the page's element is the page's
            self.by_winit.remove(&old.window.id());
            if old.appended
                && let Some(element) = old.window.canvas()
            {
                element.remove();
            }
        }
        shell.layout.add(def_id);
        if let Some(title) = self.host.window_title(def_id) {
            shell.layout.set_title(def_id, title);
        }
        self.host.set_ui_scale(def_id, shell.scale);
        let mut slot = CanvasSlot::new(shell.window.clone());
        slot.buttons = shell.buttons.clone();
        slot.mods = shell.mods.clone();
        slot.composer = compose::Composer::attach(self.id, def_id);
        self.canvases.insert(def_id, slot);
        log(&format!("def {def_id}: a pane of the dock"));
        self.pane_render(def_id);
        self.settle_dock();
        if let Some(shell) = &self.dock {
            shell.window.request_redraw();
        }
    }

    /// **Pane `def`'s title changed**: its tab says it.
    pub(super) fn retitle_pane(&mut self, def: i32) {
        let title = self.host.window_title(def);
        if let (Some(shell), Some(title)) = (self.dock.as_mut(), title)
            && shell.layout.holds(def)
        {
            shell.layout.set_title(def, title);
            self.settle_dock();
        }
    }

    /// Gives pane `def` its texture on the dock's device, once there is
    /// one, and builds its resources against it.
    fn pane_render(&mut self, def: i32) {
        let Some(shell) = self.dock.as_ref() else {
            return;
        };
        let (Some((gpu, _)), Some(rect)) = (shell.render.as_ref(), shell.layout.rect(def)) else {
            return;
        };
        let Some(slot) = self.canvases.get_mut(&def) else {
            return;
        };
        if slot.render.is_some() {
            return;
        }
        let gpu = Gpu::pane(gpu, rect.w, rect.h);
        slot.render = Some(WindowRender {
            renderers: Renderers::new(&gpu.device, gpu.target()),
            batches: Batches::new(&gpu.device, gpu.target()),
            gpu,
            waveforms: HashMap::new(),
            spectrograms: HashMap::new(),
            canvases: HashMap::new(),
        });
        let pending = std::mem::take(&mut slot.pending_bulk);
        self.build_resources(def);
        for (at, data) in pending {
            self.place_bulk_at(def, at, data);
        }
        self.request_redraw(def);
    }

    /// **Canvas slot `def` is no longer a pane**: the others share its room.
    pub(super) fn detach_pane(&mut self, def: i32) {
        let Some(shell) = self.dock.as_mut() else {
            return;
        };
        if shell.layout.remove(def) {
            shell.hidden.remove(&def);
            shell.window.request_redraw();
            self.settle_dock();
        }
    }

    /// The dock's device is up: the surface takes the canvas' size, and
    /// every pane waiting for it gets its texture.
    pub(super) fn dock_gpu_ready(&mut self, mut gpu: Gpu) {
        let Some(shell) = self.dock.as_mut() else {
            return;
        };
        let size = shell.window.inner_size();
        let (w, h) = if size.width > 0 && size.height > 0 {
            (size.width, size.height)
        } else {
            shell.pending_size.unwrap_or((size.width, size.height))
        };
        gpu.resize(w, h);
        shell.layout.set_size((w.max(1), h.max(1)));
        log(&format!(
            "dock: GPU device ready on {}; surface {}x{}",
            gpu.adapter, gpu.config.width, gpu.config.height
        ));
        let composer = Composer::new(&gpu.device, gpu.config.format);
        shell.render = Some((gpu, composer));
        let panes = shell.layout.panes();
        self.settle_dock();
        for def in panes {
            self.pane_render(def);
        }
    }

    /// **The page sized the dock's canvas**, in device pixels at the
    /// density `scale`: the surface follows, and every pane its place.
    pub(super) fn resize_dock(&mut self, width: u32, height: u32, scale: f32) {
        let Some(shell) = self.dock.as_mut() else {
            return;
        };
        let _ = shell
            .window
            .request_inner_size(PhysicalSize::new(width, height));
        match shell.render.as_mut() {
            Some((gpu, _)) => gpu.resize(width, height),
            None => shell.pending_size = Some((width, height)),
        }
        shell.layout.set_size((width.max(1), height.max(1)));
        if shell.scale != scale {
            shell.scale = scale;
            shell.layout.set_metrics(self.host.metrics.resolved(scale));
            for def in shell.layout.panes() {
                self.host.set_ui_scale(def, scale);
            }
        }
        self.settle_dock();
        if let Some(shell) = &self.dock {
            shell.window.request_redraw();
        }
    }

    /// **The dock brought up to what changed in it**: every pane at the size
    /// of its place (a pane whose rectangle changed is resized and draws
    /// again whole), a pane behind another's tab not seen and one shown again
    /// seen, and the dock drawn if anything of it moved.
    ///
    /// Answers the pane a hand dragged out of the dock, for the caller to
    /// give a canvas of its own ([`WebApp::move_window`]).
    fn settle_dock(&mut self) -> Option<i32> {
        let shell = self.dock.as_mut()?;
        let dirty = shell.layout.take_dirty();
        if dirty.layout {
            for def in shell.layout.panes() {
                let shown = shell.layout.shown(def);
                let rect = shell.layout.rect(def);
                let Some(slot) = self.canvases.get_mut(&def) else {
                    continue;
                };
                if let (Some(render), Some(rect)) = (slot.render.as_mut(), rect)
                    && (render.gpu.config.width, render.gpu.config.height) != (rect.w, rect.h)
                {
                    render.gpu.resize(rect.w, rect.h);
                    slot.request_redraw();
                }
                // a hidden tab is a canvas out of view: no tick, no frame
                if shown == shell.hidden.contains(&def) {
                    slot.visible = shown;
                    if shown {
                        shell.hidden.remove(&def);
                        slot.request_redraw();
                    } else {
                        shell.hidden.insert(def);
                    }
                }
            }
            self.on_tree_changed();
        }
        if (dirty.layout || dirty.chrome)
            && let Some(shell) = &self.dock
        {
            shell.window.request_redraw();
        }
        dirty.out
    }

    /// **Window `def` moves into the dock (`into`) or out of it** into a
    /// canvas of its own, appended to the page as a window the host opens
    /// with no place is: its GPU state goes with the surface it drew on, and
    /// it is built again where it goes -- the def and the state the host
    /// holds of it as they were, its gesture machine and its histories
    /// carried, its pictures read again.
    pub(super) fn move_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        def: i32,
        into: bool,
    ) -> Result<(), String> {
        if self.dock.is_none() {
            return Err("the dock is not open: the page has not handed it a canvas".into());
        }
        if self.is_pane(def) == into {
            return Err(if into {
                format!("window {def} is a pane already")
            } else {
                format!("window {def} is not a pane")
            });
        }
        let old = self
            .canvases
            .get_mut(&def)
            .ok_or_else(|| format!("no window {def}"))?;
        let gestures = std::mem::take(&mut old.gestures);
        let histories = std::mem::take(&mut old.histories);
        self.drop_own(def);
        if into {
            self.undocked.remove(&def);
        } else {
            self.undocked.insert(def);
        }
        self.attach(event_loop, def, None);
        if let Some(slot) = self.canvases.get_mut(&def) {
            slot.gestures = gestures;
            slot.histories = histories;
        }
        self.start_bulk(def);
        self.on_tree_changed();
        log(&format!(
            "def {def}: {}",
            if into {
                "put back in the dock"
            } else {
                "taken out of the dock"
            }
        ));
        Ok(())
    }

    /// **Window `def` leaves where it is drawn**: out of the dock if it is a
    /// pane, its own canvas off the page if this front appended one.
    pub(super) fn drop_own(&mut self, def: i32) {
        if let Some(slot) = self.canvases.remove(&def)
            && !self.is_dock_window(slot.window.id())
        {
            self.by_winit.remove(&slot.window.id());
            if slot.appended
                && let Some(element) = slot.window.canvas()
            {
                element.remove();
            }
        }
        self.detach_pane(def);
    }

    /// **What the page said of the dock's canvas**, handed to the pane
    /// it is about: the motion, the buttons and a touch to the pane under it
    /// (or the one a press holds), the keys to the pane last pressed in, what
    /// is the whole canvas' to the dock or to every pane.
    pub(super) fn dock_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        let Some(shell) = self.dock.as_mut() else {
            return;
        };
        let panes = shell.layout.panes();
        match event {
            WindowEvent::RedrawRequested => self.compose_dock(),
            WindowEvent::Resized(size) => {
                if let Some((gpu, _)) = shell.render.as_mut() {
                    gpu.resize(size.width, size.height);
                }
                shell
                    .layout
                    .set_size((size.width.max(1), size.height.max(1)));
                shell.window.request_redraw();
                self.settle_dock();
            }
            WindowEvent::ModifiersChanged(_) => {
                for def in panes {
                    self.on_def_event(def, event.clone());
                }
            }
            WindowEvent::CursorMoved {
                device_id,
                position,
            } => {
                let motion = shell.layout.moved(position.x, position.y);
                if let Some(left) = motion.left {
                    self.on_def_event(left, WindowEvent::CursorLeft { device_id });
                }
                if let Some((def, (x, y))) = motion.to {
                    let position = winit::dpi::PhysicalPosition::new(x, y);
                    self.on_def_event(
                        def,
                        WindowEvent::CursorMoved {
                            device_id,
                            position,
                        },
                    );
                }
                self.settle_dock();
            }
            WindowEvent::CursorLeft { device_id } => {
                if let Some(def) = shell.layout.left() {
                    self.on_def_event(def, WindowEvent::CursorLeft { device_id });
                }
                self.settle_dock();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pane = match (state, button) {
                    (ElementState::Pressed, MouseButton::Left) => shell.layout.pressed(),
                    (ElementState::Released, MouseButton::Left) => shell.layout.released(),
                    _ => shell.layout.pointer(),
                };
                if let Some(def) = pane {
                    self.on_def_event(def, event);
                }
                // a tab let go out of the dock: a canvas of its own
                if let Some(def) = self.settle_dock()
                    && let Err(e) = self.move_window(event_loop, def, false)
                {
                    log(&format!("dock: {e}"));
                }
            }
            WindowEvent::Touch(mut touch) => {
                // a finger goes where a pointer would: its pane, its pixels
                let (x, y) = (touch.location.x, touch.location.y);
                let pane = match touch.phase {
                    TouchPhase::Started => {
                        shell.layout.moved(x, y);
                        shell.layout.pressed()
                    }
                    TouchPhase::Ended | TouchPhase::Cancelled => shell.layout.released(),
                    TouchPhase::Moved => shell.layout.moved(x, y).to.map(|(def, _)| def),
                };
                if let Some(def) = pane
                    && let Some((lx, ly)) = shell.layout.local(def, x, y)
                {
                    touch.location = winit::dpi::PhysicalPosition::new(lx, ly);
                    self.on_def_event(def, WindowEvent::Touch(touch));
                }
                if let Some(def) = self.settle_dock()
                    && let Err(e) = self.move_window(event_loop, def, false)
                {
                    log(&format!("dock: {e}"));
                }
            }
            WindowEvent::MouseWheel { .. } => {
                if let Some(def) = shell.layout.pointer() {
                    self.on_def_event(def, event);
                }
            }
            WindowEvent::KeyboardInput { .. } => {
                if let Some(def) = shell.layout.focus() {
                    self.on_def_event(def, event);
                }
            }
            _ => {}
        }
    }

    /// **The dock's frame**: each shown pane that asked for a frame draws
    /// it into its texture, then every shown pane is copied into its place
    /// in the surface with the dock's chrome over them.
    fn compose_dock(&mut self) {
        let Some(shown) = self
            .dock
            .as_ref()
            .map(|s| s.layout.placed().map(|(def, _)| def).collect::<Vec<_>>())
        else {
            return;
        };
        for def in shown {
            if self.canvases.get(&def).is_some_and(|s| s.owed.take()) {
                self.draw(def);
            }
        }
        let clear = divider_color(&self.host.theme);
        let WebApp {
            dock,
            canvases,
            host,
            ..
        } = self;
        let Some(shell) = dock.as_mut() else {
            return;
        };
        let chrome = chrome_mesh(&shell.layout, &host.theme);
        let Some((gpu, composer)) = shell.render.as_mut() else {
            return;
        };
        let Some(surface) = gpu.surface() else {
            return;
        };
        let frame = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            _ => {
                surface.configure(&gpu.device, &gpu.config);
                shell.window.request_redraw();
                return;
            }
        };
        let target = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let views = pane_views(&shell.layout, canvases);
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("dock frame"),
            });
        composer.compose(gpu, &mut encoder, &target, clear, &views, &chrome);
        gpu.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
    }

    /// **A call to the dock** (`/gui_dock`), carried out and answered: a
    /// move of the layout, a hand on the surface, the picture (answered
    /// once it is written).
    pub(super) fn dock_call(&mut self, event_loop: &ActiveEventLoop, seq: i32, call: Call) {
        let refused = |e: String| ("command".to_string(), e);
        let answer = match (self.dock.is_some(), call) {
            (_, Call::Undock(def)) => self
                .move_window(event_loop, def, false)
                .map(|()| self.dock_state())
                .map_err(refused),
            (_, Call::Dock(def)) => self
                .move_window(event_loop, def, true)
                .map(|()| self.dock_state())
                .map_err(refused),
            (false, _) => Err((
                "runtime".to_string(),
                "the dock is not open: the page has not handed it a canvas".to_string(),
            )),
            (true, Call::Pointer { kind, x, y }) => {
                if let Some(def) = self.dock_pointer(kind, x, y)
                    && let Err(e) = self.move_window(event_loop, def, false)
                {
                    log(&format!("dock: {e}"));
                }
                Ok(self.dock_state())
            }
            (true, Call::Capture(path)) => match self.dock_capture(seq, path) {
                Ok(()) => return,
                Err(e) => Err(("runtime".to_string(), e)),
            },
            (true, call) => {
                let answer = match self.dock.as_mut() {
                    Some(shell) => shell.layout.apply(&call),
                    None => Ok(Value::Null),
                };
                self.settle_dock();
                answer.map(|_| self.dock_state())
            }
        };
        self.queue(call::reply(seq, answer));
    }

    /// The dock as it stands, with the windows taken out of it.
    fn dock_state(&self) -> Value {
        let state = self
            .dock
            .as_ref()
            .map_or_else(call::closed_state, |s| s.layout.state());
        let out: Vec<i32> = self.undocked.iter().copied().collect();
        call::with_out(state, &out)
    }

    /// **A scripted hand on the dock's surface**, at `(x, y)` of it: the
    /// chrome takes what lands on it, and a pane what lands in it, at its own
    /// pixels, through the canvas' scripted pointer.
    ///
    /// Answers a pane the hand dragged out of the dock.
    fn dock_pointer(&mut self, kind: PointerKind, x: f64, y: f64) -> Option<i32> {
        let shell = self.dock.as_mut()?;
        let layout = &mut shell.layout;
        let motion = layout.moved(x, y);
        let to = match kind {
            PointerKind::Move => motion.to.map(|(def, _)| def),
            PointerKind::Press => layout.pressed(),
            PointerKind::Release => layout.released(),
        };
        let local = to.and_then(|def| layout.local(def, x, y).map(|p| (def, p)));
        if let Some((def, (lx, ly))) = local {
            self.pointer(def, kind, lx, ly, PointerMods::default());
        }
        self.settle_dock()
    }

    /// **The dock's picture at `path`**: every shown pane drawn into its
    /// texture, composed with the chrome into a texture of the surface's
    /// size, and written into the page's storage once the card has it --
    /// answered then.
    fn dock_capture(&mut self, seq: i32, path: String) -> Result<(), String> {
        let shown: Vec<i32> = self
            .dock
            .as_ref()
            .map(|s| s.layout.placed().map(|(def, _)| def).collect())
            .unwrap_or_default();
        for def in shown {
            if let Some(slot) = self.canvases.get(&def) {
                slot.request_redraw();
                slot.owed.set(false);
            }
            self.draw(def);
        }
        let clear = divider_color(&self.host.theme);
        let host_id = self.id;
        let WebApp {
            dock,
            canvases,
            host,
            ..
        } = self;
        let shell = dock.as_mut().ok_or("the dock closed")?;
        let chrome = chrome_mesh(&shell.layout, &host.theme);
        let (gpu, composer) = shell
            .render
            .as_mut()
            .ok_or("the dock's device is not up yet")?;
        let views = pane_views(&shell.layout, canvases);
        let readback = composer.capture(gpu, clear, &views, &chrome)?;
        wasm_bindgen_futures::spawn_local(super::capture::finish(
            host_id,
            CaptureOf::Dock(seq),
            path,
            readback,
            gpu.device.clone(),
        ));
        Ok(())
    }
}

/// Every shown pane's texture with its place.
fn pane_views<'a>(
    layout: &Dock,
    canvases: &'a HashMap<i32, CanvasSlot>,
) -> Vec<(crate::host::dock::PaneRect, &'a wgpu::TextureView)> {
    layout
        .placed()
        .filter_map(|(def, rect)| {
            canvases
                .get(&def)
                .and_then(|slot| slot.render.as_ref())
                .and_then(|render| render.gpu.pane_view())
                .map(|view| (rect, view))
        })
        .collect()
}

/// The dock's chrome as one batch, drawn at its own size roles.
fn chrome_mesh(layout: &Dock, theme: &clausters_gui_draw::theme::Theme) -> Mesh {
    let mut mesh = Mesh::new();
    let metrics = *layout.metrics();
    layout.draw(&mut Draw::new(&mut mesh, &metrics, theme));
    mesh
}
