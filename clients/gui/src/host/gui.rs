//! The windowed GUI host: winit + wgpu driven by the `/gui_*` protocol.
//!
//! This is the GPU front of the host (the headless one is [`super::transport`]).
//! The OSC transport runs on a **background thread** and forwards every datagram
//! to the winit **main thread** through an [`EventLoopProxy`] (winit owns the
//! main thread; window creation must happen there). The main thread holds the
//! [`Host`] — the single source of truth for the typed widget trees — opens an OS
//! window per window-rooted GuiDef, lays each tree into rectangles
//! ([`super::layout`]) and renders it: the heavy `waveform` view into its
//! viewport (the existing [`WaveformView`](crate::waveform::WaveformView)), and
//! the control widgets and chrome through the flat-geometry painter
//! ([`super::paint`]) with bitmap text ([`super::font`]).
//!
//! Interaction closes the loop: dragging a slider/knob, clicking a button/toggle/
//! menu writes the new value back into the host's tree and emits `/gui_event` to
//! the script that built the window; closing a window emits `/gui_closed`; a live
//! `/gui_set` repaints. Only this module touches winit; a wasm build swaps it for
//! a `<canvas>` surface and the rest is unchanged.

use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;

use clausters_core::osc::{OscMessage, OscPacket, OscType, encode};
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
use super::paint::{Color, Mesh, Painter};
use super::widget::{Widget, WidgetKind};
use super::{ClientId, GUI_CLOSED, GUI_EVENT, Host, HostEffect, controls};

const CLEAR: wgpu::Color = wgpu::Color {
    r: 0.05,
    g: 0.05,
    b: 0.07,
    a: 1.0,
};
const PANEL_COLOR: Color = [0.10, 0.11, 0.14, 0.55];
const LABEL_COLOR: Color = [0.85, 0.87, 0.90, 1.0];
const LABEL_SCALE: f32 = 2.0;

/// What the background transport thread hands the main (winit) thread.
#[derive(Debug)]
pub enum UserEvent {
    /// One OSC datagram and where it came from (decoded on the main thread,
    /// through the single shared door, to keep all logic on one thread).
    Osc { from: SocketAddr, bytes: Vec<u8> },
}

/// Runs the windowed host: spawn the OSC transport thread, then own the winit
/// event loop on this (main) thread until the process is stopped.
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

/// An in-progress pointer drag, by what it is driving.
enum Drag {
    /// A horizontal slider: the value follows the cursor x within `body`.
    Slider { id: i32, body: Rect },
    /// A knob or number: the value moves with the vertical drag from a snapshot.
    Vertical {
        id: i32,
        start_fraction: f32,
        origin_y: f64,
        body_h: f32,
    },
    /// A momentary button held down (emits 0 on release).
    Button { id: i32 },
    /// Panning a waveform's time view from a snapshot.
    Waveform {
        id: i32,
        origin_x: f64,
        start: f64,
        body_w: f64,
    },
}

/// One open window: its GPU surface, the per-waveform slots, the painter, the
/// script address its events go to, and the pointer/drag state. The widget tree
/// itself lives in the [`Host`] (single source of truth).
struct WindowState {
    gpu: Gpu,
    waveforms: HashMap<i32, WaveformSlot>,
    painter: Painter,
    origin: SocketAddr,
    cursor: (f64, f64),
    drag: Option<Drag>,
}

struct App {
    host: Host,
    socket: Arc<UdpSocket>,
    windows: HashMap<i32, WindowState>,
    by_winit: HashMap<WindowId, i32>,
    /// Window opens requested before the first `resumed`, flushed on resume.
    pending: Vec<(i32, SocketAddr)>,
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

    fn apply(&mut self, event_loop: &ActiveEventLoop, from: SocketAddr, effects: Vec<HostEffect>) {
        for effect in effects {
            match effect {
                HostEffect::Reply(msg) => self.send(from, msg),
                HostEffect::OpenWindow(id) => {
                    if self.resumed {
                        self.open_window(event_loop, id, from);
                    } else {
                        self.pending.push((id, from));
                    }
                }
                HostEffect::CloseWindow(id) => self.drop_window(id),
                HostEffect::Redraw(id) => {
                    if let Some(ws) = self.windows.get(&id) {
                        ws.gpu.window.request_redraw();
                    }
                }
            }
        }
    }

    /// Encodes and sends one message to `to`.
    fn send(&self, to: SocketAddr, msg: OscMessage) {
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

    /// Emits `/gui_event widget_id <args…>` to the window's script.
    fn emit(&self, def_id: i32, widget_id: i32, mut args: Vec<OscType>) {
        let Some(ws) = self.windows.get(&def_id) else {
            return;
        };
        let mut msg_args = vec![OscType::Int(widget_id)];
        msg_args.append(&mut args);
        self.send(
            ws.origin,
            OscMessage {
                addr: GUI_EVENT.into(),
                args: msg_args,
            },
        );
    }

    /// Emits a control's current value as a `/gui_event`.
    fn emit_value(&self, def_id: i32, widget_id: i32) {
        if let Some(value) = self
            .host
            .window_def(def_id)
            .and_then(|t| value_of(t, widget_id))
        {
            self.emit(def_id, widget_id, vec![value]);
        }
    }

    fn open_window(&mut self, event_loop: &ActiveEventLoop, id: i32, origin: SocketAddr) {
        // Read the window metadata, releasing the host borrow before mutating
        // (drop_window) and before re-borrowing the tree for the waveforms.
        let Some((title, width, height)) = self.host.window_def(id).and_then(|t| match &t.kind {
            WidgetKind::Window {
                title,
                width,
                height,
                ..
            } => Some((
                title
                    .clone()
                    .unwrap_or_else(|| format!("clausters-gui {id}")),
                *width,
                *height,
            )),
            _ => None,
        }) else {
            return; // freed between the effect and now, or not a window
        };

        self.drop_window(id); // rebuild semantics on a re-/gui_def

        let attrs = Window::default_attributes()
            .with_title(title.clone())
            .with_inner_size(LogicalSize::new(width as f64, height as f64));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => return warn!("gui_def {id}: cannot create window: {e}"),
        };
        let winit_id = window.id();
        let gpu = pollster::block_on(Gpu::new(window));

        let mut waveforms = HashMap::new();
        if let Some(tree) = self.host.window_def(id) {
            collect_waveforms(tree, &gpu, &mut waveforms);
        }
        let painter = Painter::new(&gpu.device, gpu.config.format);

        self.by_winit.insert(winit_id, id);
        self.windows.insert(
            id,
            WindowState {
                gpu,
                waveforms,
                painter,
                origin,
                cursor: (0.0, 0.0),
                drag: None,
            },
        );
        info!("gui_def {id}: opened window \"{title}\"");
        if let Some(ws) = self.windows.get(&id) {
            ws.gpu.window.request_redraw();
        }
    }

    fn drop_window(&mut self, id: i32) {
        if let Some(ws) = self.windows.remove(&id) {
            self.by_winit.remove(&ws.gpu.window.id());
        }
    }

    /// User-initiated close: tell the script, then drop the window.
    fn close_by_user(&mut self, id: i32) {
        if let Some(ws) = self.windows.get(&id) {
            self.send(
                ws.origin,
                OscMessage {
                    addr: GUI_CLOSED.into(),
                    args: vec![OscType::Int(id)],
                },
            );
        }
        self.drop_window(id);
    }

    /// The framebuffer size of a window.
    fn fb(&self, def_id: i32) -> (u32, u32) {
        self.windows
            .get(&def_id)
            .map(|w| (w.gpu.config.width.max(1), w.gpu.config.height.max(1)))
            .unwrap_or((1, 1))
    }

    /// The deepest widget under `(x, y)`: its id, rect and a clone of its kind.
    fn hit(&self, def_id: i32, x: f64, y: f64) -> Option<(i32, Rect, WidgetKind)> {
        let (fb_w, fb_h) = self.fb(def_id);
        let tree = self.host.window_def(def_id)?;
        let area = Rect::new(0.0, 0.0, fb_w as f32, fb_h as f32);
        let mut found = None;
        for p in layout::layout(area, tree) {
            if p.rect.contains(x, y)
                && let Some(id) = p.widget.id
                && !matches!(
                    p.widget.kind,
                    WidgetKind::Window { .. } | WidgetKind::Panel { .. }
                )
            {
                found = Some((id, p.rect, p.widget.kind.clone()));
            }
        }
        found
    }

    /// Sets a continuous control's value from a 0..1 fraction, in the host tree.
    fn set_fraction(&mut self, def_id: i32, widget_id: i32, t: f32) {
        if let Some(tree) = self.host.window_def_mut(def_id)
            && let Some(w) = tree.find_mut(widget_id)
        {
            match &mut w.kind {
                WidgetKind::Slider(r) | WidgetKind::Knob(r) | WidgetKind::Number(r) => {
                    r.set_fraction(t)
                }
                _ => {}
            }
        }
    }

    fn redraw(&self, def_id: i32) {
        if let Some(ws) = self.windows.get(&def_id) {
            ws.gpu.window.request_redraw();
        }
    }

    /// Press on a widget: act by kind and possibly start a drag.
    fn on_press(&mut self, def_id: i32) {
        let (cx, cy) = self
            .windows
            .get(&def_id)
            .map(|w| w.cursor)
            .unwrap_or((0.0, 0.0));
        let Some((id, rect, kind)) = self.hit(def_id, cx, cy) else {
            return;
        };
        match kind {
            WidgetKind::Slider(r) => {
                let body = controls::body_rect(rect, r.label.is_some());
                let t = controls::slider_fraction(body, cx);
                self.set_fraction(def_id, id, t);
                self.emit_value(def_id, id);
                self.set_drag(def_id, Drag::Slider { id, body });
                self.redraw(def_id);
            }
            WidgetKind::Knob(r) | WidgetKind::Number(r) => {
                let body = controls::body_rect(rect, r.label.is_some());
                self.set_drag(
                    def_id,
                    Drag::Vertical {
                        id,
                        start_fraction: r.fraction(),
                        origin_y: cy,
                        body_h: body.h,
                    },
                );
            }
            WidgetKind::Button { .. } => {
                self.emit(def_id, id, vec![OscType::Int(1)]);
                self.set_drag(def_id, Drag::Button { id });
                self.redraw(def_id);
            }
            WidgetKind::Toggle { .. } => {
                self.flip_toggle(def_id, id);
                self.emit_value(def_id, id);
                self.redraw(def_id);
            }
            WidgetKind::Menu { .. } => {
                self.cycle_menu(def_id, id);
                self.emit_value(def_id, id);
                self.redraw(def_id);
            }
            WidgetKind::Waveform { .. } => {
                if let Some(slot) = self.windows.get(&def_id).and_then(|w| w.waveforms.get(&id)) {
                    self.set_drag(
                        def_id,
                        Drag::Waveform {
                            id,
                            origin_x: cx,
                            start: slot.nav.start,
                            body_w: rect.w.max(1.0) as f64,
                        },
                    );
                }
            }
            _ => {}
        }
    }

    fn set_drag(&mut self, def_id: i32, drag: Drag) {
        if let Some(ws) = self.windows.get_mut(&def_id) {
            ws.drag = Some(drag);
        }
    }

    fn flip_toggle(&mut self, def_id: i32, id: i32) {
        if let Some(tree) = self.host.window_def_mut(def_id)
            && let Some(w) = tree.find_mut(id)
            && let WidgetKind::Toggle { value, .. } = &mut w.kind
        {
            *value = !*value;
        }
    }

    fn cycle_menu(&mut self, def_id: i32, id: i32) {
        if let Some(tree) = self.host.window_def_mut(def_id)
            && let Some(w) = tree.find_mut(id)
            && let WidgetKind::Menu { index, options, .. } = &mut w.kind
            && !options.is_empty()
        {
            *index = (*index + 1) % options.len();
        }
    }

    /// Pointer moved while a drag is active: drive the dragged target.
    fn on_drag(&mut self, def_id: i32, cx: f64, cy: f64) {
        // Read the drag descriptor out (cheap copies) to release the borrow.
        let action = self
            .windows
            .get(&def_id)
            .and_then(|w| w.drag.as_ref())
            .map(|d| match d {
                Drag::Slider { id, body } => DragMove::Slider(*id, *body),
                Drag::Vertical {
                    id,
                    start_fraction,
                    origin_y,
                    body_h,
                } => DragMove::Vertical(*id, *start_fraction, *origin_y, *body_h),
                Drag::Button { .. } => DragMove::None,
                Drag::Waveform {
                    id,
                    origin_x,
                    start,
                    body_w,
                } => DragMove::Waveform(*id, *origin_x, *start, *body_w),
            });
        match action {
            Some(DragMove::Slider(id, body)) => {
                let t = controls::slider_fraction(body, cx);
                self.set_fraction(def_id, id, t);
                self.emit_value(def_id, id);
                self.redraw(def_id);
            }
            Some(DragMove::Vertical(id, start_fraction, origin_y, body_h)) => {
                let t = start_fraction + controls::drag_fraction_delta(cy - origin_y, body_h);
                self.set_fraction(def_id, id, t.clamp(0.0, 1.0));
                self.emit_value(def_id, id);
                self.redraw(def_id);
            }
            Some(DragMove::Waveform(id, origin_x, start, body_w)) => {
                self.pan_waveform(def_id, id, start, (cx - origin_x) / body_w);
            }
            Some(DragMove::None) | None => {}
        }
    }

    fn pan_waveform(&mut self, def_id: i32, id: i32, start: f64, dx_fraction: f64) {
        if let Some(ws) = self.windows.get_mut(&def_id)
            && let Some(slot) = ws.waveforms.get_mut(&id)
        {
            let total = slot.view.total_samples();
            slot.nav
                .set_start(start - dx_fraction * slot.nav.len, total);
        }
        self.emit_view(def_id, id);
        self.redraw(def_id);
    }

    /// Emits a waveform's visible range as a `/gui_event id "view" start len`.
    fn emit_view(&self, def_id: i32, id: i32) {
        if let Some(slot) = self.windows.get(&def_id).and_then(|w| w.waveforms.get(&id)) {
            self.emit(
                def_id,
                id,
                vec![
                    OscType::String("view".into()),
                    OscType::Float(slot.nav.start as f32),
                    OscType::Float(slot.nav.len as f32),
                ],
            );
        }
    }

    /// Release: a held button emits 0; any drag ends.
    fn on_release(&mut self, def_id: i32) {
        let drag = self.windows.get_mut(&def_id).and_then(|w| w.drag.take());
        if let Some(Drag::Button { id }) = drag {
            self.emit(def_id, id, vec![OscType::Int(0)]);
            self.redraw(def_id);
        }
    }

    fn render(&mut self, def_id: i32) {
        let (fb_w, fb_h) = self.fb(def_id);
        // Build the frame mesh from the host tree (immutable borrow), then the
        // window's GPU resources (mutable) upload and draw it.
        let Some(tree) = self.host.window_def(def_id) else {
            return;
        };
        let area = Rect::new(0.0, 0.0, fb_w as f32, fb_h as f32);
        let placed = layout::layout(area, tree);
        let mut mesh = Mesh::new();
        let mut waveform_rects: Vec<(i32, Rect)> = Vec::new();
        let active_button = match self.windows.get(&def_id).and_then(|w| w.drag.as_ref()) {
            Some(Drag::Button { id }) => Some(*id),
            _ => None,
        };
        for p in &placed {
            match &p.widget.kind {
                WidgetKind::Panel { .. } => mesh.rect(p.rect, PANEL_COLOR),
                WidgetKind::Label { text } => {
                    font_left(&mut mesh, text, p.rect);
                }
                WidgetKind::Waveform { .. } => {
                    if let Some(id) = p.widget.id {
                        waveform_rects.push((id, p.rect));
                    }
                }
                WidgetKind::Window { .. } | WidgetKind::Unknown(_) => {}
                kind => controls::draw(&mut mesh, kind, p.rect, p.widget.id == active_button),
            }
        }

        let Some(ws) = self.windows.get_mut(&def_id) else {
            return;
        };
        ws.painter
            .upload(&ws.gpu.device, &ws.gpu.queue, &mesh, fb_w, fb_h);
        for (id, rect) in &waveform_rects {
            if let Some(slot) = ws.waveforms.get_mut(id) {
                slot.view.upload(
                    &ws.gpu.device,
                    &ws.gpu.queue,
                    &slot.nav,
                    rect.w.max(1.0) as u32,
                );
            }
        }

        let frame = match ws.gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            _ => {
                ws.gpu.surface.configure(&ws.gpu.device, &ws.gpu.config);
                return;
            }
        };
        let target = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = ws
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
            ws.painter.draw(&mut pass);
            for (id, rect) in &waveform_rects {
                if rect.w >= 1.0
                    && rect.h >= 1.0
                    && let Some(slot) = ws.waveforms.get(id)
                {
                    let (x, y, w, h) = clamp_viewport(*rect, fb_w, fb_h);
                    pass.set_viewport(x, y, w, h, 0.0, 1.0);
                    slot.view.draw(&mut pass);
                }
            }
        }
        ws.gpu.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
    }
}

/// A drag step, copied out of the borrow so the host tree can be mutated.
enum DragMove {
    Slider(i32, Rect),
    Vertical(i32, f32, f64, f32),
    Waveform(i32, f64, f64, f64),
    None,
}

/// The current event value of widget `id` in `tree`.
fn value_of(tree: &Widget, id: i32) -> Option<OscType> {
    fn walk(w: &Widget, id: i32) -> Option<OscType> {
        if w.id == Some(id) {
            return w.kind.event_value();
        }
        w.children.iter().find_map(|c| walk(c, id))
    }
    walk(tree, id)
}

fn font_left(mesh: &mut Mesh, text: &str, rect: Rect) {
    let y = rect.y + (rect.h - super::font::height(LABEL_SCALE)) * 0.5;
    super::font::text(
        mesh,
        text,
        rect.x + 4.0,
        y.max(rect.y),
        LABEL_SCALE,
        LABEL_COLOR,
    );
}

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
        for (id, origin) in std::mem::take(&mut self.pending) {
            self.open_window(event_loop, id, origin);
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
        match event {
            WindowEvent::CloseRequested => self.close_by_user(def_id),
            WindowEvent::Resized(size) => {
                if let Some(ws) = self.windows.get_mut(&def_id) {
                    ws.gpu.resize(size.width, size.height);
                    ws.gpu.window.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(ws) = self.windows.get_mut(&def_id) {
                    ws.cursor = (position.x, position.y);
                }
                let dragging = self.windows.get(&def_id).is_some_and(|w| w.drag.is_some());
                if dragging {
                    self.on_drag(def_id, position.x, position.y);
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => match state {
                ElementState::Pressed => self.on_press(def_id),
                ElementState::Released => self.on_release(def_id),
            },
            WindowEvent::MouseWheel { delta, .. } => {
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64,
                    MouseScrollDelta::PixelDelta(p) => p.y / 50.0,
                };
                let (cx, cy) = self
                    .windows
                    .get(&def_id)
                    .map(|w| w.cursor)
                    .unwrap_or((0.0, 0.0));
                if let Some((id, rect, WidgetKind::Waveform { .. })) = self.hit(def_id, cx, cy) {
                    self.zoom_waveform(def_id, id, rect, cx, 0.85f64.powf(steps));
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.logical_key {
                    Key::Named(NamedKey::Escape) => self.close_by_user(def_id),
                    Key::Character(ref c) if c.eq_ignore_ascii_case("r") => {
                        self.reset_waveforms(def_id)
                    }
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => self.render(def_id),
            _ => {}
        }
    }
}

impl App {
    fn zoom_waveform(&mut self, def_id: i32, id: i32, rect: Rect, cx: f64, factor: f64) {
        if let Some(ws) = self.windows.get_mut(&def_id)
            && let Some(slot) = ws.waveforms.get_mut(&id)
        {
            let total = slot.view.total_samples();
            let anchor = ((cx - rect.x as f64) / rect.w.max(1.0) as f64).clamp(0.0, 1.0);
            slot.nav.zoom(factor, anchor, total);
        }
        self.emit_view(def_id, id);
        self.redraw(def_id);
    }

    fn reset_waveforms(&mut self, def_id: i32) {
        let ids: Vec<i32> = self
            .windows
            .get(&def_id)
            .map(|w| w.waveforms.keys().copied().collect())
            .unwrap_or_default();
        if let Some(ws) = self.windows.get_mut(&def_id) {
            for slot in ws.waveforms.values_mut() {
                slot.nav = View::full(slot.view.total_samples());
            }
            ws.gpu.window.request_redraw();
        }
        for id in ids {
            self.emit_view(def_id, id);
        }
    }
}
