//! The windowed GUI host: winit + wgpu driven by the `/gui_*` protocol.
//!
//! This is the GPU front of the host (the headless one is [`super::transport`]).
//! The OSC transport runs on a **background thread** and forwards every datagram
//! to the winit **main thread** through an [`EventLoopProxy`] (winit owns the
//! main thread; window creation must happen there). The main thread holds the
//! [`Host`], opens an OS window per window-rooted GuiDef, lays each window's
//! widget tree into rectangles ([`super::layout`]) and renders them: the heavy
//! `waveform` view into its rectangle's viewport (the existing
//! [`WaveformView`](crate::waveform::WaveformView), navigable with the same
//! wheel-zoom / drag-pan / `R`-reset bindings as the prototype), and
//! panels/labels as flat chrome rectangles ([`super::rects`]). Replies (e.g.
//! `/gui_info`) go back out the shared socket to the requester.
//!
//! Only this module touches winit; a wasm build swaps it for a `<canvas>`
//! surface and the rest is unchanged.

use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;

use clausters_core::osc::{OscMessage, OscPacket, encode};
use tracing::{info, warn};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::native::Gpu;
use crate::view::TimelineView;
use crate::viewport::View;
use crate::waveform::{WaveformData, WaveformView};

use super::layout::{self, Rect};
use super::rects::RectRenderer;
use super::widget::{Widget, WidgetKind};
use super::{ClientId, Host, HostEffect};

/// The window background and the chrome colors (RGBA, premultiplied alpha-blend).
const CLEAR: wgpu::Color = wgpu::Color {
    r: 0.05,
    g: 0.05,
    b: 0.07,
    a: 1.0,
};
const PANEL_COLOR: [f32; 4] = [0.12, 0.13, 0.17, 0.6];
const LABEL_COLOR: [f32; 4] = [0.22, 0.24, 0.30, 0.7];

/// What the background transport thread hands the main (winit) thread.
#[derive(Debug)]
pub enum UserEvent {
    /// One OSC datagram and where it came from (decoded on the main thread,
    /// through the single shared door, to keep all logic on one thread).
    Osc { from: SocketAddr, bytes: Vec<u8> },
}

/// Runs the windowed host: spawn the OSC transport thread, then own the winit
/// event loop on this (main) thread until the process is stopped. `socket` is
/// shared — the transport thread receives on it, the main thread replies on it.
pub fn run(host: Host, socket: Arc<UdpSocket>) -> Result<(), String> {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|e| format!("cannot create the window event loop ({e}); use --headless on a machine with no display"))?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();
    let recv_socket = Arc::clone(&socket);
    std::thread::Builder::new()
        .name("clausters-gui-osc".into())
        .spawn(move || transport_loop(recv_socket, proxy))
        .map_err(|e| e.to_string())?;

    let mut app = App::new(host, socket);
    event_loop.run_app(&mut app).map_err(|e| e.to_string())
}

/// Receives datagrams and forwards them to the event loop until it goes away.
fn transport_loop(socket: Arc<UdpSocket>, proxy: EventLoopProxy<UserEvent>) {
    let mut buf = vec![0u8; 65536];
    loop {
        match socket.recv_from(&mut buf) {
            Ok((0, _)) => {}
            Ok((len, from)) => {
                let event = UserEvent::Osc {
                    from,
                    bytes: buf[..len].to_vec(),
                };
                if proxy.send_event(event).is_err() {
                    return; // the event loop has exited
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {}
            Err(_) => return,
        }
    }
}

/// A waveform widget's GPU view plus its own navigation window.
struct WaveformSlot {
    view: WaveformView,
    nav: View,
}

/// One open window: its GPU surface, the typed tree it renders, the per-waveform
/// slots, the chrome renderer and the pointer/drag state for navigation.
struct WindowState {
    gpu: Gpu,
    tree: Widget,
    waveforms: HashMap<i32, WaveformSlot>,
    rects: RectRenderer,
    cursor: (f64, f64),
    dragging: bool,
    drag_origin_x: f64,
    drag_start: f64,
    drag_target: Option<i32>,
    drag_rect_w: f64,
}

struct App {
    host: Host,
    socket: Arc<UdpSocket>,
    windows: HashMap<i32, WindowState>,
    by_winit: HashMap<WindowId, i32>,
    /// Window opens requested before the first `resumed` (when the event loop is
    /// not yet ready to create windows), flushed on resume.
    pending: Vec<i32>,
    resumed: bool,
}

impl App {
    fn new(host: Host, socket: Arc<UdpSocket>) -> Self {
        Self {
            host,
            socket,
            windows: HashMap::new(),
            by_winit: HashMap::new(),
            pending: Vec::new(),
            resumed: false,
        }
    }

    /// Acts on the effects of handling one packet: send replies to `from`, open
    /// and close windows.
    fn apply(&mut self, event_loop: &ActiveEventLoop, from: SocketAddr, effects: Vec<HostEffect>) {
        for effect in effects {
            match effect {
                HostEffect::Reply(msg) => self.reply(from, msg),
                HostEffect::OpenWindow(id) => {
                    if self.resumed {
                        self.open_window(event_loop, id);
                    } else {
                        self.pending.push(id);
                    }
                }
                HostEffect::CloseWindow(id) => self.close_window(id),
            }
        }
    }

    fn reply(&self, to: SocketAddr, msg: OscMessage) {
        let addr = msg.addr.clone();
        match encode(&OscPacket::Message(msg)) {
            Ok(bytes) => {
                if let Err(e) = self.socket.send_to(&bytes, to) {
                    warn!("failed to send {addr} to {to}: {e}");
                }
            }
            Err(e) => warn!("failed to encode {addr}: {e}"),
        }
    }

    /// Opens (or rebuilds) the window for window-rooted def `id`.
    fn open_window(&mut self, event_loop: &ActiveEventLoop, id: i32) {
        let Some(tree) = self.host.window_def(id).cloned() else {
            return; // freed between the effect and now
        };
        let WidgetKind::Window {
            title,
            width,
            height,
            ..
        } = &tree.kind
        else {
            return;
        };
        let title = title
            .clone()
            .unwrap_or_else(|| format!("clausters-gui {id}"));
        let (width, height) = (*width, *height);

        // Rebuild semantics: a re-`/gui_def` of the same window id replaces it.
        self.close_window(id);

        let attrs = Window::default_attributes()
            .with_title(title)
            .with_inner_size(LogicalSize::new(width as f64, height as f64));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => return warn!("gui_def {id}: cannot create window: {e}"),
        };
        let winit_id = window.id();
        let gpu = pollster::block_on(Gpu::new(window));

        let mut waveforms = HashMap::new();
        collect_waveforms(&tree, &gpu, &mut waveforms);
        let rects = RectRenderer::new(&gpu.device, gpu.config.format);

        self.by_winit.insert(winit_id, id);
        self.windows.insert(
            id,
            WindowState {
                gpu,
                tree,
                waveforms,
                rects,
                cursor: (0.0, 0.0),
                dragging: false,
                drag_origin_x: 0.0,
                drag_start: 0.0,
                drag_target: None,
                drag_rect_w: 1.0,
            },
        );
        info!("gui_def {id}: opened window \"{}\"", self.window_title(id));
        if let Some(ws) = self.windows.get(&id) {
            ws.gpu.window.request_redraw();
        }
    }

    fn window_title(&self, id: i32) -> String {
        match self.windows.get(&id).map(|w| &w.tree.kind) {
            Some(WidgetKind::Window { title: Some(t), .. }) => t.clone(),
            _ => format!("clausters-gui {id}"),
        }
    }

    fn close_window(&mut self, id: i32) {
        if let Some(ws) = self.windows.remove(&id) {
            self.by_winit.remove(&ws.gpu.window.id());
        }
    }
}

/// Builds a `WaveformSlot` for every waveform widget in the tree (keyed by id).
fn collect_waveforms(widget: &Widget, gpu: &Gpu, out: &mut HashMap<i32, WaveformSlot>) {
    if let WidgetKind::Waveform {
        samples,
        base_bucket,
    } = &widget.kind
        && let Some(id) = widget.id
    {
        let data = WaveformData::new(Arc::clone(samples), *base_bucket);
        let nav = View::full(data.total_samples());
        let view = WaveformView::new(&gpu.device, gpu.config.format, data);
        out.insert(id, WaveformSlot { view, nav });
    }
    for child in &widget.children {
        collect_waveforms(child, gpu, out);
    }
}

/// One item to render: its rectangle and what to draw there. Owned, so the
/// layout borrow on the tree is released before the GPU upload borrows the
/// window's other fields.
enum DrawKind {
    Panel,
    Label,
    Waveform(i32),
}

fn draw_kind(widget: &Widget) -> Option<DrawKind> {
    match (&widget.kind, widget.id) {
        (WidgetKind::Panel { .. }, _) => Some(DrawKind::Panel),
        (WidgetKind::Label { .. }, _) => Some(DrawKind::Label),
        (WidgetKind::Waveform { .. }, Some(id)) => Some(DrawKind::Waveform(id)),
        _ => None, // window root and unknown types: laid out but not painted
    }
}

impl WindowState {
    /// Lays out the tree into the current framebuffer and renders one frame.
    fn render(&mut self) {
        let (fb_w, fb_h) = (self.gpu.config.width.max(1), self.gpu.config.height.max(1));
        let area = Rect::new(0.0, 0.0, fb_w as f32, fb_h as f32);
        // Snapshot the layout into owned items so the tree borrow is released.
        let items: Vec<(Rect, DrawKind)> = layout::layout(area, &self.tree)
            .into_iter()
            .filter_map(|p| draw_kind(p.widget).map(|k| (p.rect, k)))
            .collect();

        // Chrome (panels/labels) batched into one buffer.
        let chrome: Vec<(Rect, [f32; 4])> = items
            .iter()
            .filter_map(|(rect, kind)| match kind {
                DrawKind::Panel => Some((*rect, PANEL_COLOR)),
                DrawKind::Label => Some((*rect, LABEL_COLOR)),
                DrawKind::Waveform(_) => None,
            })
            .collect();
        self.rects
            .upload(&self.gpu.device, &self.gpu.queue, &chrome, fb_w, fb_h);

        // Each waveform resolves to exactly its rectangle's pixel width.
        for (rect, kind) in &items {
            if let DrawKind::Waveform(id) = kind
                && let Some(slot) = self.waveforms.get_mut(id)
            {
                slot.view.upload(
                    &self.gpu.device,
                    &self.gpu.queue,
                    &slot.nav,
                    rect.w.max(1.0) as u32,
                );
            }
        }

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
        let target = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("gui frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gui pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(CLEAR),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            // Chrome first (full-framebuffer viewport), then each waveform in its
            // own viewport so it draws in its own clip space.
            self.rects.draw(&mut pass);
            for (rect, kind) in &items {
                if let DrawKind::Waveform(id) = kind
                    && rect.w >= 1.0
                    && rect.h >= 1.0
                    && let Some(slot) = self.waveforms.get(id)
                {
                    let (x, y, w, h) = clamp_viewport(*rect, fb_w, fb_h);
                    pass.set_viewport(x, y, w, h, 0.0, 1.0);
                    slot.view.draw(&mut pass);
                }
            }
        }
        self.gpu.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
    }

    /// The waveform widget (id and rect) under `(x, y)`, if any.
    fn waveform_at(&self, x: f64, y: f64) -> Option<(i32, Rect)> {
        let (fb_w, fb_h) = (self.gpu.config.width.max(1), self.gpu.config.height.max(1));
        let area = Rect::new(0.0, 0.0, fb_w as f32, fb_h as f32);
        layout::layout(area, &self.tree).into_iter().find_map(|p| {
            match (&p.widget.kind, p.widget.id) {
                (WidgetKind::Waveform { .. }, Some(id)) if p.rect.contains(x, y) => {
                    Some((id, p.rect))
                }
                _ => None,
            }
        })
    }
}

/// Clamps a layout rectangle to the framebuffer, so `set_viewport` never exceeds
/// the surface (a sub-pixel overhang from rounding would be a validation error).
fn clamp_viewport(r: Rect, fb_w: u32, fb_h: u32) -> (f32, f32, f32, f32) {
    let x = r.x.clamp(0.0, fb_w as f32);
    let y = r.y.clamp(0.0, fb_h as f32);
    let w = r.w.min(fb_w as f32 - x).max(0.0);
    let h = r.h.min(fb_h as f32 - y).max(0.0);
    (x, y, w, h)
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.resumed = true;
        for id in std::mem::take(&mut self.pending) {
            self.open_window(event_loop, id);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        let UserEvent::Osc { from, bytes } = event;
        let packet = match clausters_core::osc::decode_packet(&bytes) {
            Ok(p) => p,
            Err(e) => return warn!("malformed OSC packet from {from}: {e}"),
        };
        let effects = self.host.handle_packet(packet, ClientId::Udp(from));
        self.apply(event_loop, from, effects);
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(&def_id) = self.by_winit.get(&window_id) else {
            return;
        };
        let Some(ws) = self.windows.get_mut(&def_id) else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                self.close_window(def_id);
            }
            WindowEvent::Resized(size) => {
                ws.gpu.resize(size.width, size.height);
                ws.gpu.window.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                ws.cursor = (position.x, position.y);
                if ws.dragging
                    && let Some(id) = ws.drag_target
                    && let Some(slot) = ws.waveforms.get_mut(&id)
                {
                    let total = slot.nav.len;
                    let dx = (position.x - ws.drag_origin_x) / ws.drag_rect_w;
                    slot.nav
                        .set_start(ws.drag_start - dx * slot.nav.len, total as usize);
                    ws.gpu.window.request_redraw();
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                ws.dragging = state == ElementState::Pressed;
                if ws.dragging {
                    let (cx, cy) = ws.cursor;
                    if let Some((id, rect)) = ws.waveform_at(cx, cy) {
                        ws.drag_target = Some(id);
                        ws.drag_origin_x = cx;
                        ws.drag_rect_w = rect.w.max(1.0) as f64;
                        ws.drag_start = ws.waveforms.get(&id).map_or(0.0, |s| s.nav.start);
                    } else {
                        ws.drag_target = None;
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64,
                    MouseScrollDelta::PixelDelta(p) => p.y / 50.0,
                };
                let factor = 0.85f64.powf(steps);
                let (cx, cy) = ws.cursor;
                if let Some((id, rect)) = ws.waveform_at(cx, cy)
                    && let Some(slot) = ws.waveforms.get_mut(&id)
                {
                    let total = slot.view.total_samples();
                    let anchor = ((cx - rect.x as f64) / rect.w.max(1.0) as f64).clamp(0.0, 1.0);
                    slot.nav.zoom(factor, anchor, total);
                    ws.gpu.window.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.logical_key {
                    Key::Named(NamedKey::Escape) => self.close_window(def_id),
                    Key::Character(ref c) if c.eq_ignore_ascii_case("r") => {
                        for slot in ws.waveforms.values_mut() {
                            slot.nav = View::full(slot.view.total_samples());
                        }
                        ws.gpu.window.request_redraw();
                    }
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => ws.render(),
            _ => {}
        }
    }
}
