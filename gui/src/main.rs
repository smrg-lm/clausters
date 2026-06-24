//! Native entry point for the waveform prototype.
//!
//! This window is only a *driver* for `WaveformRenderer`: it owns a wgpu
//! surface tied to a `winit` window and forwards input (scroll = zoom around
//! the cursor, drag = pan) into the renderer's `View`. The renderer itself is
//! windowing-agnostic, so the exact same `waveform.rs`/`waveform.wgsl` pair is
//! what a browser build would drive against a `<canvas>` WebGPU surface.
//!
//! Run with `cargo run` (needs a display and a Vulkan/Metal/DX12/GL adapter).
//! Controls: mouse wheel zooms toward the pointer, left-drag pans, `R` resets,
//! `Esc` quits.

mod waveform;

use std::f64::consts::PI;
use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use waveform::{Envelope, View, WaveformRenderer};

const SAMPLE_RATE: f64 = 48_000.0;
const SIGNAL_LEN: usize = 4_000_000; // ~83 s of mono audio.
const BASE_BUCKET: usize = 256;

/// A frequency sweep with a slow tremolo envelope plus a little noise, so that
/// zooming in reveals real structure (individual cycles) and zooming out shows
/// the amplitude bursts. No `rand` dependency: a tiny xorshift suffices.
fn make_test_signal(n: usize) -> Vec<f32> {
    let mut v = Vec::with_capacity(n);
    let mut phase = 0.0f64;
    let mut rng: u64 = 0x2545_F491_4F6C_DD1D;
    for i in 0..n {
        let t = i as f64 / SAMPLE_RATE;
        let f = 80.0 + (4000.0 - 80.0) * (i as f64 / n as f64);
        phase += 2.0 * PI * f / SAMPLE_RATE;
        let env = (0.5 + 0.5 * (2.0 * PI * 0.5 * t).sin()).powi(2);
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        let noise = (rng >> 40) as f64 / (1u64 << 24) as f64 - 0.5;
        v.push(((phase.sin() * 0.8 + noise * 0.05) * env) as f32);
    }
    v
}

/// wgpu surface + device bound to a winit window.
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
        // Native default instance (all backends). A web build would instead use
        // `wgpu::util::new_instance_with_webgpu_detection` to target WebGPU.
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
                label: Some("waveform device"),
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
    renderer: WaveformRenderer,
    env: Envelope,
    view: View,
    cursor_x: f64,
    dragging: bool,
    last_drag_x: f64,
}

impl State {
    fn new(gpu: Gpu) -> Self {
        let samples = make_test_signal(SIGNAL_LEN);
        let env = Envelope::build(&samples, BASE_BUCKET);
        let renderer = WaveformRenderer::new(&gpu.device, gpu.config.format);
        let view = View {
            start: 0.0,
            len: env.total_samples() as f64,
        };
        Self {
            gpu,
            renderer,
            env,
            view,
            cursor_x: 0.0,
            dragging: false,
            last_drag_x: 0.0,
        }
    }

    fn render(&mut self) {
        self.renderer.upload_geometry(
            &self.gpu.device,
            &self.gpu.queue,
            &self.env,
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
                label: Some("waveform pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.07,
                            g: 0.07,
                            b: 0.09,
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
            self.renderer.draw(&mut pass);
        }
        self.gpu.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
    }
}

#[derive(Default)]
struct App {
    state: Option<State>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Clausters - waveform prototype (wheel: zoom, drag: pan, R: reset)");
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        let gpu = pollster::block_on(Gpu::new(window));
        self.state = Some(State::new(gpu));
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        let total = state.env.total_samples();
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
                let factor = 0.85f64.powf(steps);
                state.view.zoom(factor, anchor, total);
                state.gpu.window.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.logical_key {
                    Key::Named(NamedKey::Escape) => event_loop.exit(),
                    Key::Character(ref c) if c.as_str() == "r" || c.as_str() == "R" => {
                        state.view = View {
                            start: 0.0,
                            len: total as f64,
                        };
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

fn main() {
    let event_loop = EventLoop::new().expect("create event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App::default();
    event_loop.run_app(&mut app).expect("run app");
}
