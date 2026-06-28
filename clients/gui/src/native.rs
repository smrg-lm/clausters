//! Native (winit + wgpu) windowing harness shared by the view prototypes.
//!
//! It owns the window, surface and event loop, and drives any [`TimelineView`]
//! with one set of input bindings (wheel = zoom toward the cursor, drag = pan,
//! `R` = reset, `Esc` = quit). Everything view-specific lives behind the trait,
//! so adding a new view is a new binary plus a factory, not new windowing code.
//! Excluded from wasm builds, where a `<canvas>` surface replaces winit.
#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::gpu::Gpu;
use crate::view::TimelineView;
use crate::viewport::View;

/// Builds the view once the GPU device exists (it needs the device, queue and
/// surface format to create pipelines and upload data/textures).
pub type ViewFactory =
    Box<dyn FnOnce(&wgpu::Device, &wgpu::Queue, wgpu::TextureFormat) -> Box<dyn TimelineView>>;

struct State {
    gpu: Gpu,
    view_obj: Box<dyn TimelineView>,
    view: View,
    cursor_x: f64,
    cursor_y: f64,
    dragging: bool,
    shift: bool,
    // Absolute-drag anchor: cursor position and time-view start at mouse-down
    // (or at a Shift change mid-drag). Panning recomputes from these so a
    // clamped edge never accumulates drift.
    drag_origin_x: f64,
    drag_origin_y: f64,
    drag_view_start: f64,
}

impl State {
    fn new(gpu: Gpu, factory: ViewFactory) -> Self {
        let view_obj = factory(&gpu.device, &gpu.queue, gpu.config.format);
        let view = View::full(view_obj.total_samples());
        Self {
            gpu,
            view_obj,
            view,
            cursor_x: 0.0,
            cursor_y: 0.0,
            dragging: false,
            shift: false,
            drag_origin_x: 0.0,
            drag_origin_y: 0.0,
            drag_view_start: 0.0,
        }
    }

    /// Re-anchor an absolute drag to the current cursor and view state.
    fn anchor_drag(&mut self) {
        self.drag_origin_x = self.cursor_x;
        self.drag_origin_y = self.cursor_y;
        self.drag_view_start = self.view.start;
        self.view_obj.on_vertical_drag_begin();
    }

    fn render(&mut self) {
        self.view_obj.upload(
            &self.gpu.device,
            &self.gpu.queue,
            &self.view,
            self.gpu.config.width,
        );
        let frame = match self.gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            _ => {
                self.gpu
                    .surface
                    .configure(&self.gpu.device, &self.gpu.config);
                return;
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("view pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.05,
                            g: 0.05,
                            b: 0.07,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.view_obj.draw(&mut pass);
        }
        self.gpu.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
    }
}

struct App {
    title: String,
    factory: Option<ViewFactory>,
    state: Option<State>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title(self.title.clone());
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        let gpu = pollster::block_on(Gpu::new(window));
        let factory = self.factory.take().expect("factory consumed once");
        self.state = Some(State::new(gpu, factory));
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        let total = state.view_obj.total_samples();
        let (w, h) = (
            state.gpu.config.width.max(1) as f64,
            state.gpu.config.height.max(1) as f64,
        );
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                state.gpu.resize(size.width, size.height);
                state.gpu.window.request_redraw();
            }
            WindowEvent::ModifiersChanged(mods) => {
                let new_shift = mods.state().shift_key();
                // Switching axis mid-drag: re-anchor so it does not jump.
                if new_shift != state.shift && state.dragging {
                    state.anchor_drag();
                }
                state.shift = new_shift;
            }
            WindowEvent::CursorMoved { position, .. } => {
                state.cursor_x = position.x;
                state.cursor_y = position.y;
                if state.dragging {
                    let redraw = if state.shift {
                        // Absolute vertical drag from the snapshot (low at bottom,
                        // so dragging down moves the view down with the cursor).
                        let total_y = (position.y - state.drag_origin_y) / h;
                        state.view_obj.on_vertical_drag(total_y)
                    } else {
                        let total_x = (position.x - state.drag_origin_x) / w;
                        state
                            .view
                            .set_start(state.drag_view_start - total_x * state.view.len, total);
                        true
                    };
                    if redraw {
                        state.gpu.window.request_redraw();
                    }
                }
            }
            WindowEvent::MouseInput {
                state: btn_state,
                button: MouseButton::Left,
                ..
            } => {
                state.dragging = btn_state == ElementState::Pressed;
                if state.dragging {
                    state.anchor_drag();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64,
                    MouseScrollDelta::PixelDelta(p) => p.y / 50.0,
                };
                let factor = 0.85f64.powf(steps);
                let redraw = if state.shift {
                    // anchor measured from the bottom for the frequency axis.
                    let anchor = (1.0 - state.cursor_y / h).clamp(0.0, 1.0);
                    state.view_obj.on_vertical_zoom(factor, anchor)
                } else {
                    let anchor = (state.cursor_x / w).clamp(0.0, 1.0);
                    state.view.zoom(factor, anchor, total);
                    true
                };
                if redraw {
                    state.gpu.window.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.logical_key {
                    Key::Named(NamedKey::Escape) => event_loop.exit(),
                    Key::Character(ref c) if c.eq_ignore_ascii_case("r") => {
                        state.view = View::full(total);
                        state.gpu.window.request_redraw();
                    }
                    Key::Character(ref c) => {
                        if let Some(ch) = c.chars().next()
                            && state.view_obj.on_char(ch)
                        {
                            state.gpu.window.request_redraw();
                        }
                    }
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => state.render(),
            _ => {}
        }
    }
}

/// Open a window titled `title` showing the view built by `factory`, and run
/// until closed. Blocks the calling thread (it owns the event loop).
pub fn run(title: impl Into<String>, factory: ViewFactory) {
    let event_loop = EventLoop::new().expect("create event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App {
        title: title.into(),
        factory: Some(factory),
        state: None,
    };
    event_loop.run_app(&mut app).expect("run app");
}
