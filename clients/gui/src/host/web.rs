//! The browser entry point: first pixels over WebGPU.
//!
//! This is the wasm twin of the native windowed front ([`super::gui`]). It
//! creates a winit window over an HTML `<canvas>`, brings up the wgpu device
//! **asynchronously** (no `block_on` — the browser's main thread must never
//! block), and renders a **compiled-in GuiDef** through the *same* shared
//! [`super::frame::render`] the desktop uses, so the result is pixel-faithful by
//! construction rather than a parallel renderer.
//!
//! There is no transport yet: G12 isolates the surface/GPU/loop port from the
//! protocol, so the tree is built in Rust (parsed from a JSON GuiDef exactly as a
//! client would send it) and meters read zero. The WebSocket carrier that drives
//! the host live arrives in G13.

#![cfg(target_arch = "wasm32")]

use std::collections::HashMap;
use std::sync::Arc;

use wasm_bindgen::prelude::wasm_bindgen;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::platform::web::{EventLoopExtWebSys, WindowAttributesExtWebSys};
use winit::window::{Window, WindowId};

use crate::gpu::Gpu;
use crate::waveform::WaveformData;

use super::canvas::CanvasView;
use super::frame::{self, WaveformSlot};
use super::guidef::GuiNode;
use super::paint::Painter;
use super::widget::{Widget, WidgetKind};

/// Logs a line to the browser console (the wasm `tracing` shim is out of scope
/// for G12; one or two startup lines are enough to confirm bring-up).
fn log(msg: &str) {
    web_sys::console::log_1(&msg.into());
}

/// The single user event: the async GPU finished initializing and is ready to
/// render. winit's web event loop is single-threaded, so moving the `Gpu` (which
/// owns non-`Send` WebGPU handles) through the proxy is fine.
enum WebEvent {
    GpuReady(Gpu),
}

/// The per-window GPU resources, the wasm counterpart of the native
/// `WindowState` (minus the transport/interaction state G12 does not have yet).
struct WindowRender {
    gpu: Gpu,
    painter: Painter,
    waveforms: HashMap<i32, WaveformSlot>,
    canvases: HashMap<i32, CanvasView>,
}

/// The browser app: it holds the compiled-in widget tree and, once the GPU is
/// up, the window's render resources.
struct WebApp {
    proxy: EventLoopProxy<WebEvent>,
    tree: Widget,
    size: (u32, u32),
    window: Option<Arc<Window>>,
    render: Option<WindowRender>,
}

impl WebApp {
    fn new(proxy: EventLoopProxy<WebEvent>) -> Self {
        let tree = demo_guidef();
        let size = match &tree.kind {
            WidgetKind::Window { width, height, .. } => (*width, *height),
            _ => (640, 360),
        };
        Self {
            proxy,
            tree,
            size,
            window: None,
            render: None,
        }
    }

    fn redraw(&self) {
        if let Some(render) = self.render.as_ref() {
            render.gpu.window.request_redraw();
        }
    }
}

impl ApplicationHandler<WebEvent> for WebApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // `with_append(true)` makes winit create the `<canvas>` and append it to
        // the document body, so the quick-start HTML needs no canvas of its own.
        let attrs = Window::default_attributes()
            .with_title("clausters-gui (web)")
            .with_inner_size(LogicalSize::new(self.size.0 as f64, self.size.1 as f64))
            .with_append(true);
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                log(&format!("cannot create the canvas window: {e}"));
                return;
            }
        };
        self.window = Some(window.clone());
        log("opened window over <canvas>; requesting WebGPU adapter...");
        // Bring up wgpu asynchronously and hand the device back through the proxy
        // — never block the browser's main thread.
        let proxy = self.proxy.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let gpu = Gpu::new(window).await;
            let _ = proxy.send_event(WebEvent::GpuReady(gpu));
        });
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: WebEvent) {
        match event {
            WebEvent::GpuReady(gpu) => {
                let painter = Painter::new(&gpu.device, gpu.config.format);
                let mut waveforms = HashMap::new();
                build_inline_waveforms(&self.tree, &gpu, &mut waveforms);
                let canvases = HashMap::new();
                log("WebGPU ready; rendering the GuiDef");
                self.render = Some(WindowRender {
                    gpu,
                    painter,
                    waveforms,
                    canvases,
                });
                self.redraw();
            }
        }
    }

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(render) = self.render.as_mut() else {
            return;
        };
        match event {
            WindowEvent::Resized(size) => {
                render.gpu.resize(size.width, size.height);
                render.gpu.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                // The very same shared frame path the native host draws — fed the
                // empty inputs (no bus, no node tree, no held button) since G12
                // has no transport.
                let scopes = HashMap::new();
                frame::render(
                    &mut render.gpu,
                    &mut render.painter,
                    &mut render.waveforms,
                    &mut render.canvases,
                    &scopes,
                    &self.tree,
                    &frame::FrameInputs::default(),
                );
            }
            _ => {}
        }
    }
}

/// Builds the GPU slot for every inline-data `waveform` in the tree (the only
/// bulk source available in the browser at G12; the `path`/`cache`/`buffer`
/// sources are the native mmap path and the later network fallback).
fn build_inline_waveforms(widget: &Widget, gpu: &Gpu, out: &mut HashMap<i32, WaveformSlot>) {
    if let WidgetKind::Waveform {
        samples,
        base_bucket,
        ..
    } = &widget.kind
        && let Some(id) = widget.id
        && !samples.is_empty()
    {
        let data = WaveformData::new(Arc::clone(samples), *base_bucket);
        out.insert(id, frame::waveform_slot(data, gpu));
    }
    for child in &widget.children {
        build_inline_waveforms(child, gpu, out);
    }
}

/// The compiled-in GuiDef: a panel of controls plus a `waveform` from inline
/// data, authored as the same JSON a client would send over `/gui_def` and built
/// through the unchanged construction path ([`GuiNode::parse`] +
/// [`Widget::from_node`]).
fn demo_guidef() -> Widget {
    // A short synthetic signal so the heavy waveform view has something to draw.
    const N: usize = 4096;
    let data: Vec<f32> = (0..N)
        .map(|i| {
            let t = i as f32 / N as f32;
            (t * std::f32::consts::TAU * 6.0).sin() * (1.0 - t) * 0.85
        })
        .collect();
    let tree = serde_json::json!({
        "type": "window", "title": "clausters-gui (web)", "w": 480, "h": 380, "layout": "col",
        "children": [
            {"id": 1, "type": "label", "text": "clausters-gui in the browser"},
            {"id": 2, "type": "slider", "label": "cutoff", "min": 20.0, "max": 20000.0, "value": 800.0},
            {"id": 3, "type": "knob", "label": "res", "min": 0.0, "max": 1.0, "value": 0.3},
            {"id": 4, "type": "toggle", "label": "gate", "value": 1},
            {"id": 5, "type": "button", "label": "ping"},
            {"id": 6, "type": "waveform", "data": data, "base_bucket": 64}
        ]
    });
    let bytes = serde_json::to_vec(&tree).expect("serialize the demo GuiDef");
    let node = GuiNode::parse(&bytes).expect("parse the demo GuiDef");
    Widget::from_node(0, &node, &[]).expect("build the demo GuiDef tree")
}

/// The wasm entry point (called from the JS loader). Sets the panic hook, builds
/// the event loop and spawns the app on the browser's animation-frame loop —
/// `spawn_app` returns immediately, so nothing blocks the main thread.
#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    let event_loop = EventLoop::<WebEvent>::with_user_event()
        .build()
        .expect("build the web event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    log("clausters-gui web host starting");
    event_loop.spawn_app(WebApp::new(proxy));
}
