//! **The dock, natively**: one window of the platform whose surface
//! shows every window the host opens as a pane of it
//! ([`crate::host::dock`]), when the host was asked for one
//! (`Host::dock`).
//!
//! A pane is a window in every way but its surface: it has its `WindowState`,
//! its gesture machine and its replies, and [`App::def_event`] is what it
//! hears. What this module adds is the one window and its routing -- the
//! platform's events arrive for the dock's window, and each goes to the
//! pane it reaches, in that pane's own pixels, or to the dock's own chrome
//! (a tab, a divider) -- the frame that copies every shown pane into the
//! surface with the chrome over it, and the dock's door on the wire
//! (`/gui_dock`).

use std::collections::HashSet;
use std::sync::Arc;

use clausters_gui_draw::draw::Draw;
use clausters_gui_draw::paint::Mesh;
use serde_json::{Value, json};
use tracing::{info, warn};
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::gpu::Gpu;
use crate::host::dock::call::{self, Call};
use crate::host::dock::{Composer, Dock, divider_color};
use crate::host::{ClientId, PointerKind, PointerMods, png};

use super::Loop;
use super::app::App;

/// The size a dock's window opens at, in logical pixels.
const OPENS_AT: (u32, u32) = (1280, 800);

/// The dock's window, its surface and its panes.
pub(in crate::host::gui) struct Shell {
    gpu: Gpu,
    layout: Dock,
    composer: Composer,
    /// The window's display scale, which every pane is laid out at.
    scale: f32,
    /// The panes behind another's tab, told they are not seen.
    hidden: HashSet<i32>,
}

impl App {
    /// Whether `id` is the dock's window.
    pub(super) fn is_dock_window(&self, id: WindowId) -> bool {
        self.dock
            .as_ref()
            .and_then(|s| s.gpu.window())
            .is_some_and(|w| w.id() == id)
    }

    /// Whether window `def_id` is a pane of the dock.
    pub(super) fn is_pane(&self, def_id: i32) -> bool {
        self.dock.as_ref().is_some_and(|s| s.layout.holds(def_id))
    }

    /// **The GPU of window `id` as a pane of the dock**, the dock's
    /// window opened first if it is not open yet. The panes already there
    /// make room for it.
    pub(super) fn open_pane(&mut self, lp: Loop, id: i32) -> Option<Gpu> {
        if self.dock.is_none() {
            self.dock = Some(self.open_shell(lp)?);
        }
        let title = self.host.window_title(id);
        let shell = self.dock.as_mut()?;
        shell.layout.add(id);
        if let Some(title) = title {
            shell.layout.set_title(id, title);
        }
        self.host.set_ui_scale(id, shell.scale);
        let rect = shell.layout.rect(id)?;
        let gpu = Gpu::pane(&shell.gpu, rect.w, rect.h);
        shell.gpu.request_redraw();
        self.settle_dock();
        Some(gpu)
    }

    /// **Pane `id`'s title changed**: its tab says it.
    pub(super) fn retitle_pane(&mut self, id: i32) {
        let title = self.host.window_title(id);
        if let (Some(shell), Some(title)) = (self.dock.as_mut(), title)
            && shell.layout.holds(id)
        {
            shell.layout.set_title(id, title);
            self.settle_dock();
        }
    }

    /// **Window `id` is no longer a pane**: the others share its room, and
    /// the dock's window closes with its last pane.
    pub(super) fn close_pane(&mut self, id: i32) {
        let Some(shell) = self.dock.as_mut() else {
            return;
        };
        if !shell.layout.remove(id) {
            return;
        }
        shell.hidden.remove(&id);
        if shell.layout.panes().is_empty() {
            self.dock = None;
            return;
        }
        self.settle_dock();
    }

    /// The dock's window and its GPU: on the screen, or offscreen for a
    /// headless host, at the size a dock opens at.
    fn open_shell(&mut self, lp: Loop) -> Option<Shell> {
        let (gpu, scale) = match lp {
            Loop::Shown(event_loop) => {
                let attrs = Window::default_attributes()
                    .with_title("clausters")
                    .with_inner_size(LogicalSize::new(
                        f64::from(OPENS_AT.0),
                        f64::from(OPENS_AT.1),
                    ));
                let window = match event_loop.create_window(attrs) {
                    Ok(w) => Arc::new(w),
                    Err(e) => {
                        warn!("dock: cannot create its window: {e}");
                        return None;
                    }
                };
                let scale = window.scale_factor() as f32;
                match pollster::block_on(Gpu::new(window, self.host.msaa)) {
                    Ok(gpu) => (gpu, scale),
                    Err(e) => {
                        warn!("dock: cannot start the GPU: {e}");
                        return None;
                    }
                }
            }
            Loop::Headless(_) => {
                match pollster::block_on(Gpu::offscreen(OPENS_AT.0, OPENS_AT.1, self.host.msaa)) {
                    Ok(gpu) => (gpu, 1.0),
                    Err(e) => {
                        warn!("dock: cannot start the GPU: {e}");
                        return None;
                    }
                }
            }
        };
        let size = (gpu.config.width.max(1), gpu.config.height.max(1));
        let metrics = self.host.metrics.resolved(scale);
        info!("dock: opened at {}x{} on {}", size.0, size.1, gpu.adapter);
        Some(Shell {
            composer: Composer::new(&gpu.device, gpu.config.format),
            layout: Dock::new(size, metrics),
            gpu,
            scale,
            hidden: HashSet::new(),
        })
    }

    /// **The dock brought up to what changed in it**: every pane at the size
    /// of its place (a pane whose rectangle changed is resized and draws
    /// again whole), a pane behind another's tab told it is not seen and one
    /// shown again told it is, and the dock drawn if anything of it moved.
    fn settle_dock(&mut self) {
        let Some(shell) = self.dock.as_mut() else {
            return;
        };
        let dirty = shell.layout.take_dirty();
        if dirty.layout {
            for id in shell.layout.panes() {
                let shown = shell.layout.shown(id);
                let rect = shell.layout.rect(id);
                let Some(ws) = self.windows.get_mut(&id) else {
                    continue;
                };
                if let Some(rect) = rect
                    && (ws.gpu.config.width, ws.gpu.config.height) != (rect.w, rect.h)
                {
                    ws.gpu.resize(rect.w, rect.h);
                    ws.repaint();
                }
                // a hidden tab is a covered window: no tick, no frame
                if shown == shell.hidden.contains(&id) {
                    ws.occluded = !shown;
                    if shown {
                        shell.hidden.remove(&id);
                        ws.repaint();
                    } else {
                        shell.hidden.insert(id);
                    }
                }
            }
        }
        if dirty.layout || dirty.chrome {
            shell.gpu.request_redraw();
        }
    }

    /// **What the platform said of the dock's window**, handed to the
    /// pane it is about: the motion and the buttons to the pane under the
    /// pointer (or the one a press holds), the keys to the pane last pressed
    /// in, what is the whole window's -- its size, its scale, the modifiers,
    /// being seen -- to the dock or to every pane.
    pub(super) fn dock_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        let Some(shell) = self.dock.as_mut() else {
            return;
        };
        let panes = shell.layout.panes().to_vec();
        match event {
            WindowEvent::RedrawRequested => self.compose_dock(),
            WindowEvent::CloseRequested => {
                for id in panes {
                    self.user_close(id, Loop::Shown(event_loop));
                }
            }
            WindowEvent::Resized(size) => {
                shell.gpu.resize(size.width, size.height);
                shell
                    .layout
                    .set_size((size.width.max(1), size.height.max(1)));
                shell.gpu.request_redraw();
                self.settle_dock();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                shell.scale = scale_factor as f32;
                shell
                    .layout
                    .set_metrics(self.host.metrics.resolved(shell.scale));
                for &id in &panes {
                    self.host.set_ui_scale(id, scale_factor as f32);
                    if let Some(ws) = self.windows.get(&id) {
                        ws.repaint();
                    }
                }
                self.settle_dock();
            }
            WindowEvent::ModifiersChanged(_)
            | WindowEvent::Occluded(_)
            | WindowEvent::Focused(_) => {
                for id in panes {
                    self.def_event(event_loop, id, event.clone());
                }
            }
            WindowEvent::CursorMoved {
                device_id,
                position,
            } => {
                let motion = shell.layout.moved(position.x, position.y);
                if let Some(left) = motion.left {
                    self.def_event(event_loop, left, WindowEvent::CursorLeft { device_id });
                }
                if let Some((id, (x, y))) = motion.to {
                    let position = PhysicalPosition::new(x, y);
                    self.def_event(
                        event_loop,
                        id,
                        WindowEvent::CursorMoved {
                            device_id,
                            position,
                        },
                    );
                }
                self.settle_dock();
            }
            WindowEvent::CursorLeft { device_id } => {
                if let Some(id) = shell.layout.left() {
                    self.def_event(event_loop, id, WindowEvent::CursorLeft { device_id });
                }
                self.settle_dock();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                // the primary button's press holds its pane until it comes up
                let pane = match (state, button) {
                    (ElementState::Pressed, MouseButton::Left) => shell.layout.pressed(),
                    (ElementState::Released, MouseButton::Left) => shell.layout.released(),
                    _ => shell.layout.pointer(),
                };
                if let Some(id) = pane {
                    self.def_event(event_loop, id, event);
                }
                self.settle_dock();
            }
            WindowEvent::MouseWheel { .. } => {
                if let Some(id) = shell.layout.pointer() {
                    self.def_event(event_loop, id, event);
                }
            }
            WindowEvent::KeyboardInput { .. } | WindowEvent::Ime(_) => {
                if let Some(id) = shell.layout.focus() {
                    self.def_event(event_loop, id, event);
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
            .map(|s| s.layout.placed().map(|(id, _)| id).collect::<Vec<_>>())
        else {
            return;
        };
        for id in shown {
            if self
                .windows
                .get(&id)
                .is_some_and(|ws| ws.asked.get().is_some())
            {
                self.render(id);
            }
        }
        let clear = divider_color(&self.host.theme);
        let App {
            dock,
            windows,
            host,
            ..
        } = self;
        let Some(shell) = dock.as_mut() else {
            return;
        };
        let Some(surface) = shell.gpu.surface() else {
            return;
        };
        let frame = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            _ => {
                // no drawable this turn: configured again and asked again,
                // as a window of its own does
                surface.configure(&shell.gpu.device, &shell.gpu.config);
                shell.gpu.request_redraw();
                return;
            }
        };
        let target = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let chrome = chrome_mesh(&shell.layout, &host.theme);
        let views: Vec<_> = shell
            .layout
            .placed()
            .filter_map(|(id, rect)| {
                windows
                    .get(&id)
                    .and_then(|ws| ws.gpu.pane_view())
                    .map(|view| (rect, view))
            })
            .collect();
        let mut encoder =
            shell
                .gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("dock frame"),
                });
        shell
            .composer
            .compose(&shell.gpu, &mut encoder, &target, clear, &views, &chrome);
        shell.gpu.queue.submit(std::iter::once(encoder.finish()));
        if let Some(window) = shell.gpu.window() {
            window.pre_present_notify();
        }
        frame.present();
    }

    /// **A call to the dock** (`/gui_dock`), carried out and answered to
    /// `from`: a move of the layout, a hand on the surface, the picture.
    pub(super) fn dock_call(&mut self, from: ClientId, seq: i32, call: Call) {
        let answer = match (self.dock.as_mut(), call) {
            (None, _) => Err((
                "runtime".to_string(),
                "the dock is not open: it opens with its first pane".to_string(),
            )),
            (Some(_), Call::Pointer { kind, x, y }) => {
                self.dock_pointer(kind, x, y);
                Ok(self.dock_state())
            }
            (Some(_), Call::Capture(path)) => self
                .dock_capture(&path)
                .map(|()| json!({ "path": path }))
                .map_err(|e| ("runtime".to_string(), e)),
            (Some(shell), call) => {
                let answer = shell.layout.apply(&call);
                self.settle_dock();
                answer
            }
        };
        self.send(from, call::reply(seq, answer));
    }

    fn dock_state(&self) -> Value {
        self.dock.as_ref().map_or(Value::Null, |s| s.layout.state())
    }

    /// **A scripted hand on the dock's surface**, at `(x, y)` of it: the
    /// chrome takes what lands on it, and a pane what lands in it, at its own
    /// pixels, through the window's scripted pointer -- the hand the
    /// platform's pointer is.
    fn dock_pointer(&mut self, kind: PointerKind, x: f64, y: f64) {
        let Some(shell) = self.dock.as_mut() else {
            return;
        };
        let layout = &mut shell.layout;
        let motion = layout.moved(x, y);
        let to = match kind {
            PointerKind::Move => motion.to.map(|(id, _)| id),
            PointerKind::Press => layout.pressed(),
            PointerKind::Release => layout.released(),
        };
        let local = to.and_then(|id| layout.local(id, x, y).map(|p| (id, p)));
        if let Some((id, (lx, ly))) = local {
            self.pointer(id, kind, lx, ly, PointerMods::default());
        }
        self.settle_dock();
    }

    /// **The dock's picture at `path`**: every shown pane drawn into its
    /// texture, then composed with the chrome into a texture of the
    /// surface's size, read back and written as a PNG.
    fn dock_capture(&mut self, path: &str) -> Result<(), String> {
        let shown: Vec<i32> = self
            .dock
            .as_ref()
            .map(|s| s.layout.placed().map(|(id, _)| id).collect())
            .unwrap_or_default();
        for &id in &shown {
            if let Some(ws) = self.windows.get(&id) {
                ws.batches.want_whole();
            }
            self.render(id);
        }
        let clear = divider_color(&self.host.theme);
        let App {
            dock,
            windows,
            host,
            ..
        } = self;
        let shell = dock.as_mut().ok_or("the dock closed")?;
        let chrome = chrome_mesh(&shell.layout, &host.theme);
        let views: Vec<_> = shell
            .layout
            .placed()
            .filter_map(|(id, rect)| {
                windows
                    .get(&id)
                    .and_then(|ws| ws.gpu.pane_view())
                    .map(|view| (rect, view))
            })
            .collect();
        let readback = shell.composer.capture(&shell.gpu, clear, &views, &chrome)?;
        readback.wait(&shell.gpu.device)?;
        let (width, height, rgba) = readback.pixels();
        std::fs::write(path, png::encode_rgba(width, height, &rgba)).map_err(|e| e.to_string())
    }
}

/// The dock's chrome as one batch, drawn at its own size roles.
fn chrome_mesh(layout: &Dock, theme: &clausters_gui_draw::theme::Theme) -> Mesh {
    let mut mesh = Mesh::new();
    let metrics = *layout.metrics();
    layout.draw(&mut Draw::new(&mut mesh, &metrics, theme));
    mesh
}
