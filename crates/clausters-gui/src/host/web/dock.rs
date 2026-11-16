//! **The dock, in a page**: one canvas whose surface shows every window
//! the host opens as a pane of it ([`crate::host::dock`]), once the page
//! handed one over ([`GuiBridge::dock`](super::bridge::GuiBridge)).
//!
//! The browser twin of the native front's `gui::dock`, which this build
//! does not compile: a pane is a [`CanvasSlot`] in every way but its canvas --
//! it draws with the dock's canvas window, into a texture of its own on
//! the dock's device -- and [`WebApp::on_def_event`] is what it hears.
//! What this adds is the canvas, its routing and the frame that copies every
//! pane into its surface.

use super::canvas::{PointerListener, watch_buttons};
use super::*;
use crate::host::dock::{Composer, Dock, divider_color};

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
        let divider = self.host.metrics.resolved(scale).divider_w as u32;
        self.dock = Some(WebShell {
            window: window.clone(),
            render: None,
            layout: Dock::new((1, 1), divider),
            scale,
            buttons,
            mods,
            _listener: listener,
            pending_size: None,
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
        self.host.set_ui_scale(def_id, shell.scale);
        let mut slot = CanvasSlot::new(shell.window.clone());
        slot.buttons = shell.buttons.clone();
        slot.mods = shell.mods.clone();
        slot.composer = compose::Composer::attach(self.id, def_id);
        self.canvases.insert(def_id, slot);
        log(&format!("def {def_id}: a pane of the dock"));
        self.pane_render(def_id);
        self.arrange_panes();
        if let Some(shell) = &self.dock {
            shell.window.request_redraw();
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
            shell.window.request_redraw();
            self.arrange_panes();
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
        let panes = shell.layout.panes().to_vec();
        self.arrange_panes();
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
            shell
                .layout
                .set_divider(self.host.metrics.resolved(scale).divider_w as u32);
            for def in shell.layout.panes().to_vec() {
                self.host.set_ui_scale(def, scale);
            }
        }
        self.arrange_panes();
        if let Some(shell) = &self.dock {
            shell.window.request_redraw();
        }
    }

    /// **Every pane at the size of its place**: a pane whose rectangle
    /// changed is resized, and draws again whole.
    fn arrange_panes(&mut self) {
        let Some(shell) = self.dock.as_ref() else {
            return;
        };
        let placed: Vec<_> = shell.layout.placed().collect();
        for (def, rect) in placed {
            let Some(slot) = self.canvases.get_mut(&def) else {
                continue;
            };
            if let Some(render) = slot.render.as_mut()
                && (render.gpu.config.width, render.gpu.config.height) != (rect.w, rect.h)
            {
                render.gpu.resize(rect.w, rect.h);
                slot.request_redraw();
            }
        }
    }

    /// **What the page said of the dock's canvas**, handed to the pane
    /// it is about: the motion, the buttons and a touch to the pane under it
    /// (or the one a press holds), the keys to the pane last pressed in, what
    /// is the whole canvas' to the dock or to every pane.
    pub(super) fn dock_event(&mut self, event: WindowEvent) {
        let Some(shell) = self.dock.as_mut() else {
            return;
        };
        let panes = shell.layout.panes().to_vec();
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
                self.arrange_panes();
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
            }
            WindowEvent::CursorLeft { device_id } => {
                if let Some(def) = shell.layout.left() {
                    self.on_def_event(def, WindowEvent::CursorLeft { device_id });
                }
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

    /// **The dock's frame**: each pane draws what it was asked for into
    /// its texture, then every pane is copied into its place in the surface.
    fn compose_dock(&mut self) {
        let Some(panes) = self.dock.as_ref().map(|s| s.layout.panes().to_vec()) else {
            return;
        };
        for &def in &panes {
            self.draw(def);
        }
        let clear = divider_color(&self.host.theme);
        let WebApp { dock, canvases, .. } = self;
        let Some(shell) = dock.as_mut() else {
            return;
        };
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
        let views: Vec<_> = shell
            .layout
            .placed()
            .filter_map(|(def, rect)| {
                canvases
                    .get(&def)
                    .and_then(|slot| slot.render.as_ref())
                    .and_then(|render| render.gpu.pane_view())
                    .map(|view| (rect, view))
            })
            .collect();
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("dock frame"),
            });
        composer.compose(gpu, &mut encoder, &target, clear, &views);
        gpu.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
    }
}
