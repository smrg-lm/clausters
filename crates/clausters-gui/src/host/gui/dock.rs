//! **The dock, natively**: one window of the platform whose surface
//! shows every window the host opens as a pane of it
//! ([`crate::host::dock`]), when the host was asked for one
//! (`Host::dock`).
//!
//! A pane is a window in every way but its surface: it has its `WindowState`,
//! its gesture machine and its replies, and [`App::def_event`] is what it
//! hears. What this module adds is the one window and its routing -- the
//! platform's events arrive for the dock's window, and each goes to the
//! pane it reaches, in that pane's own pixels -- and the frame that copies
//! every pane into the surface.

use std::sync::Arc;

use tracing::{info, warn};
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::gpu::Gpu;
use crate::host::dock::{Composer, Dock, divider_color};

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
        let shell = self.dock.as_mut()?;
        shell.layout.add(id);
        self.host.set_ui_scale(id, shell.scale);
        let rect = shell.layout.rect(id)?;
        let gpu = Gpu::pane(&shell.gpu, rect.w, rect.h);
        shell.gpu.request_redraw();
        self.arrange_panes();
        Some(gpu)
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
        if shell.layout.panes().is_empty() {
            self.dock = None;
            return;
        }
        shell.gpu.request_redraw();
        self.arrange_panes();
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
        let divider = self.host.metrics.resolved(scale).divider_w as u32;
        info!("dock: opened at {}x{} on {}", size.0, size.1, gpu.adapter);
        Some(Shell {
            composer: Composer::new(&gpu.device, gpu.config.format),
            layout: Dock::new(size, divider),
            gpu,
            scale,
        })
    }

    /// **Every pane at the size of its place**: a pane whose rectangle
    /// changed is resized, and draws again whole.
    fn arrange_panes(&mut self) {
        let Some(shell) = self.dock.as_ref() else {
            return;
        };
        let placed: Vec<_> = shell.layout.placed().collect();
        for (id, rect) in placed {
            if let Some(ws) = self.windows.get_mut(&id)
                && (ws.gpu.config.width, ws.gpu.config.height) != (rect.w, rect.h)
            {
                ws.gpu.resize(rect.w, rect.h);
                ws.repaint();
            }
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
                self.arrange_panes();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                shell.scale = scale_factor as f32;
                let divider = self.host.metrics.resolved(shell.scale).divider_w as u32;
                shell.layout.set_divider(divider);
                for &id in &panes {
                    self.host.set_ui_scale(id, scale_factor as f32);
                    if let Some(ws) = self.windows.get(&id) {
                        ws.repaint();
                    }
                }
                self.arrange_panes();
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
            }
            WindowEvent::CursorLeft { device_id } => {
                if let Some(id) = shell.layout.left() {
                    self.def_event(event_loop, id, WindowEvent::CursorLeft { device_id });
                }
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

    /// **The dock's frame**: each pane draws what it was asked for into
    /// its texture, then every pane is copied into its place in the surface,
    /// the dividers between them in their own color.
    fn compose_dock(&mut self) {
        let Some(panes) = self.dock.as_ref().map(|s| s.layout.panes().to_vec()) else {
            return;
        };
        for &id in &panes {
            self.render(id);
        }
        let clear = divider_color(&self.host.theme);
        let App { dock, windows, .. } = self;
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
            .compose(&shell.gpu, &mut encoder, &target, clear, &views);
        shell.gpu.queue.submit(std::iter::once(encoder.finish()));
        if let Some(window) = shell.gpu.window() {
            window.pre_present_notify();
        }
        frame.present();
    }
}
