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

use crate::view::TimelineView;
use crate::viewport::View;

/// Builds the view once the GPU device exists (it needs the device, queue and
/// surface format to create pipelines and upload data/textures).
pub type ViewFactory =
    Box<dyn FnOnce(&wgpu::Device, &wgpu::Queue, wgpu::TextureFormat) -> Box<dyn TimelineView>>;

struct Gpu {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
}

impl Gpu {
    async fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window.clone())
            .expect("create surface");
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("no suitable GPU adapter");
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("clausters-gui device"),
                ..Default::default()
            })
            .await
            .expect("request device");
        let config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("surface unsupported by adapter");
        surface.configure(&device, &config);
        Self {
            window,
            surface,
            device,
            queue,
            config,
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
        }
    }
}

struct State {
    gpu: Gpu,
    view_obj: Box<dyn TimelineView>,
    view: View,
    cursor_x: f64,
    dragging: bool,
    last_drag_x: f64,
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
            dragging: false,
            last_drag_x: 0.0,
        }
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
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                state.gpu.resize(size.width, size.height);
                state.gpu.window.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                if state.dragging {
                    let dx_px = position.x - state.last_drag_x;
                    let dx_frac = -dx_px / state.gpu.config.width.max(1) as f64;
                    state.view.pan(dx_frac, total);
                    state.gpu.window.request_redraw();
                }
                state.cursor_x = position.x;
                state.last_drag_x = position.x;
            }
            WindowEvent::MouseInput {
                state: btn_state,
                button: MouseButton::Left,
                ..
            } => {
                state.dragging = btn_state == ElementState::Pressed;
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64,
                    MouseScrollDelta::PixelDelta(p) => p.y / 50.0,
                };
                let anchor =
                    (state.cursor_x / state.gpu.config.width.max(1) as f64).clamp(0.0, 1.0);
                state.view.zoom(0.85f64.powf(steps), anchor, total);
                state.gpu.window.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.logical_key {
                    Key::Named(NamedKey::Escape) => event_loop.exit(),
                    Key::Character(ref c) if c.eq_ignore_ascii_case("r") => {
                        state.view = View::full(total);
                        state.gpu.window.request_redraw();
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
