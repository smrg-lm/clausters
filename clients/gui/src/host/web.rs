//! The browser entry point: a live GUI host driven over the binding surface and
//! WebSocket.
//!
//! This is the wasm twin of the native windowed front ([`super::gui`]). It runs
//! the **real** [`Host`] (the same protocol dispatch, tree, bindings and
//! `forward`), renders through the shared [`super::frame::render`], and handles
//! pointer interaction through the shared [`super::interact`] primitives — so a
//! browser window opens, updates and emits events exactly as the desktop does.
//! Only the carrier and the page glue are new:
//!
//! - a **binding surface** ([`GuiBridge`]) the in-page JS feeds OSC packets into
//!   (a `/gui_def`, `/gui_set`, `/gui_bind`, …) and drains `/gui_event`/
//!   `/gui_closed`/`/gui_info` out of, all as raw OSC bytes through the one
//!   [`decode_packet`](clausters_core::osc::decode_packet)/encode door;
//! - a [`WsServerLink`]: the host's audio-server leg over the browser-native
//!   `WebSocket` to a `--ws` server, so a bound widget bypasses the script in the
//!   browser too.
//!
//! There is no in-process engine in the browser (that is native-only); the
//! browser always drives a *separate* `--ws` audio server.

#![cfg(target_arch = "wasm32")]

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::Arc;

use clausters_core::osc::{OscMessage, OscPacket, OscType, decode_packet, encode};
use wasm_bindgen::prelude::*;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::platform::web::{EventLoopExtWebSys, WindowAttributesExtWebSys};
use winit::window::{Window, WindowId};

use crate::gpu::Gpu;
use crate::waveform::WaveformData;

use super::frame::{self, WaveformSlot};
use super::interact;
use super::layout::Rect;
use super::paint::Painter;
use super::widget::{Widget, WidgetKind};
use super::{ClientId, GUI_EVENT, Host, HostEffect, ServerLink, controls};

/// The default canvas size; a fed GuiDef lays out into it (the layout uses the
/// framebuffer size, not the def's declared `w`/`h`).
const CANVAS_SIZE: (u32, u32) = (480, 420);

/// Logs a line to the browser console (a full `tracing` wasm shim is out of scope).
fn log(msg: &str) {
    web_sys::console::log_1(&msg.into());
}

/// Writes a status line into the page's `#note` element (if present), so a
/// failure the user must act on (no WebGPU) is visible without the console.
fn set_status(msg: &str) {
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("note"))
    {
        el.set_text_content(Some(msg));
    }
}

/// The host's audio-server leg over a browser `WebSocket` to a `--ws` server.
/// Send-only at this milestone (bound-widget values); replies are a later
/// milestone. Frames sent before the socket opens are buffered and flushed on
/// open, so a `connect` immediately followed by interaction does not drop values.
pub struct WsServerLink {
    socket: web_sys::WebSocket,
    open: Rc<Cell<bool>>,
    pending: Rc<RefCell<Vec<Vec<u8>>>>,
}

impl WsServerLink {
    /// Opens a WebSocket to `url` (e.g. `ws://127.0.0.1:57120`).
    pub fn connect(url: &str) -> Result<Self, String> {
        let socket = web_sys::WebSocket::new(url).map_err(|e| format!("{e:?}"))?;
        socket.set_binary_type(web_sys::BinaryType::Arraybuffer);
        let open = Rc::new(Cell::new(false));
        let pending: Rc<RefCell<Vec<Vec<u8>>>> = Rc::new(RefCell::new(Vec::new()));
        // On open: mark open and flush whatever was buffered before the handshake.
        let socket2 = socket.clone();
        let open2 = open.clone();
        let pending2 = pending.clone();
        let on_open = Closure::<dyn FnMut()>::new(move || {
            open2.set(true);
            for frame in pending2.borrow_mut().drain(..) {
                let _ = socket2.send_with_u8_array(&frame);
            }
            log("audio-server WebSocket open");
        });
        socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        on_open.forget();
        Ok(Self {
            socket,
            open,
            pending,
        })
    }

    /// Sends one OSC message as a binary frame (one packet per frame, the G1
    /// wire format), buffering until the socket is open.
    pub fn send(&self, msg: OscMessage) -> std::io::Result<()> {
        let bytes = encode(&OscPacket::Message(msg))
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        if self.open.get() {
            self.socket
                .send_with_u8_array(&bytes)
                .map_err(|e| std::io::Error::other(format!("{e:?}")))?;
        } else {
            self.pending.borrow_mut().push(bytes);
        }
        Ok(())
    }
}

/// What the binding surface and the async GPU hand the running app, through the
/// winit web event loop's proxy (single-threaded, so the non-`Send` `Gpu` and
/// the byte buffers move freely).
enum WebEvent {
    /// The async WebGPU device is ready.
    GpuReady(Gpu),
    /// One inbound OSC packet from the in-page binding surface (a `/gui_*`).
    Inbound(Vec<u8>),
    /// Attach the audio-server leg to this `--ws` URL (for bound widgets).
    ConnectServer(String),
}

/// An in-progress pointer drag in the browser window.
enum Drag {
    Slider { id: i32, body: Rect, vertical: bool },
    Vertical { id: i32, last_y: f64, body_h: f32 },
    Button { id: i32 },
}

/// The per-window GPU resources (the browser has a single window/canvas).
struct WindowRender {
    gpu: Gpu,
    painter: Painter,
    waveforms: HashMap<i32, WaveformSlot>,
}

/// The browser host application: the live [`Host`], the window/GPU resources, the
/// pointer state, and the shared outbox the binding surface drains.
struct WebApp {
    host: Host,
    outbox: Rc<RefCell<VecDeque<Vec<u8>>>>,
    window: Option<Arc<Window>>,
    render: Option<WindowRender>,
    /// The window-rooted def currently shown (the browser shows one at a time).
    current_def: Option<i32>,
    cursor: (f64, f64),
    drag: Option<Drag>,
}

impl WebApp {
    fn new(outbox: Rc<RefCell<VecDeque<Vec<u8>>>>) -> Self {
        Self {
            host: Host::new(),
            outbox,
            window: None,
            render: None,
            current_def: None,
            cursor: (0.0, 0.0),
            drag: None,
        }
    }

    /// Handles one inbound OSC packet (from the binding surface) through the real
    /// protocol dispatch, then carries out the effects: open/redraw the window,
    /// queue replies for the page to drain.
    fn on_inbound(&mut self, bytes: &[u8]) {
        let packet = match decode_packet(bytes) {
            Ok(p) => p,
            Err(e) => return log(&format!("malformed OSC packet from the page: {e}")),
        };
        for effect in self.host.handle_packet(packet, ClientId::Web) {
            match effect {
                HostEffect::Reply(msg) => self.queue(msg),
                HostEffect::OpenWindow(id) => {
                    log(&format!("/gui_def {id}: window opened from the page"));
                    self.current_def = Some(id);
                    if self.render.is_some() {
                        self.build_resources();
                        self.request_redraw();
                    }
                }
                HostEffect::CloseWindow(id) => {
                    if self.current_def == Some(id) {
                        self.current_def = None;
                        if let Some(r) = self.render.as_mut() {
                            r.waveforms.clear();
                        }
                        self.request_redraw();
                    }
                }
                HostEffect::Redraw(id) => {
                    if self.current_def == Some(id) {
                        self.request_redraw();
                    }
                }
            }
        }
    }

    /// Encodes `msg` and pushes it to the outbox for the page to drain; also logs
    /// a short summary so events are visible without a JS OSC decoder.
    fn queue(&self, msg: OscMessage) {
        log(&format!("-> {} {:?}", msg.addr, msg.args));
        match encode(&OscPacket::Message(msg)) {
            Ok(bytes) => self.outbox.borrow_mut().push_back(bytes),
            Err(e) => log(&format!("failed to encode an outbound packet: {e}")),
        }
    }

    /// (Re)builds the GPU resources for the current def: the inline-data waveform
    /// views (the only bulk source in the browser until the network path lands).
    fn build_resources(&mut self) {
        let Some(def) = self.current_def else { return };
        let Some(render) = self.render.as_ref() else {
            return;
        };
        let Some(tree) = self.host.window_def(def) else {
            return;
        };
        let mut waveforms = HashMap::new();
        build_inline_waveforms(tree, &render.gpu, &mut waveforms);
        if let Some(render) = self.render.as_mut() {
            render.waveforms = waveforms;
        }
    }

    /// Renders the current def through the shared frame path (empty live inputs:
    /// no bus, no node tree, no held button beyond the one tracked locally).
    fn draw(&mut self) {
        let Some(def) = self.current_def else { return };
        let active_button = match &self.drag {
            Some(Drag::Button { id }) => Some(*id),
            _ => None,
        };
        let Some(tree) = self.host.window_def(def) else {
            return;
        };
        let inputs = frame::FrameInputs {
            active_button,
            ..Default::default()
        };
        let scopes = HashMap::new();
        let Some(render) = self.render.as_mut() else {
            return;
        };
        let mut canvases = HashMap::new();
        frame::render(
            &mut render.gpu,
            &mut render.painter,
            &mut render.waveforms,
            &mut canvases,
            &scopes,
            tree,
            &inputs,
        );
    }

    fn fb(&self) -> (u32, u32) {
        self.render
            .as_ref()
            .map(|r| (r.gpu.config.width.max(1), r.gpu.config.height.max(1)))
            .unwrap_or((1, 1))
    }

    /// Schedules a repaint through winit's redraw request (drawing happens in
    /// `RedrawRequested`, the idiomatic path on the browser's animation frame).
    fn request_redraw(&self) {
        if let Some(render) = self.render.as_ref() {
            render.gpu.window.request_redraw();
        } else if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }

    /// Pointer press: hit-test and act by widget kind (the shared
    /// [`interact`] primitives), then deliver the value (bound → server, else a
    /// `/gui_event` to the page).
    fn on_press(&mut self) {
        let Some(def) = self.current_def else { return };
        let (fb_w, fb_h) = self.fb();
        let (cx, cy) = self.cursor;
        let Some((id, rect, kind)) = interact::hit(&self.host, def, fb_w, fb_h, cx, cy) else {
            return;
        };
        match kind {
            WidgetKind::Slider { range: r, vertical } => {
                let body = controls::body_rect(rect, r.label.is_some());
                let t = interact::slider_t(body, cx, cy, vertical);
                interact::set_fraction(&mut self.host, def, id, t);
                self.deliver_value(def, id);
                self.drag = Some(Drag::Slider { id, body, vertical });
                self.request_redraw();
            }
            WidgetKind::Knob(r) | WidgetKind::Number(r) => {
                let body = controls::body_rect(rect, r.label.is_some());
                self.drag = Some(Drag::Vertical {
                    id,
                    last_y: cy,
                    body_h: body.h,
                });
            }
            WidgetKind::Button { .. } => {
                self.deliver(def, id, OscType::Int(1));
                self.drag = Some(Drag::Button { id });
                self.request_redraw();
            }
            WidgetKind::Toggle { .. } => {
                interact::flip_toggle(&mut self.host, def, id);
                self.deliver_value(def, id);
                self.request_redraw();
            }
            WidgetKind::Menu { .. } => {
                interact::cycle_menu(&mut self.host, def, id);
                self.deliver_value(def, id);
                self.request_redraw();
            }
            _ => {}
        }
    }

    /// Pointer move while dragging: drive the dragged control.
    fn on_move(&mut self) {
        let Some(def) = self.current_def else { return };
        let (cx, cy) = self.cursor;
        match &self.drag {
            Some(Drag::Slider { id, body, vertical }) => {
                let (id, body, vertical) = (*id, *body, *vertical);
                let t = interact::slider_t(body, cx, cy, vertical);
                interact::set_fraction(&mut self.host, def, id, t);
                self.deliver_value(def, id);
                self.request_redraw();
            }
            Some(Drag::Vertical { id, last_y, body_h }) => {
                let (id, last_y, body_h) = (*id, *last_y, *body_h);
                let cur = interact::fraction_of(&self.host, def, id).unwrap_or(0.0);
                let t = (cur + controls::drag_fraction_delta(cy - last_y, body_h)).clamp(0.0, 1.0);
                interact::set_fraction(&mut self.host, def, id, t);
                if let Some(Drag::Vertical { last_y, .. }) = self.drag.as_mut() {
                    *last_y = cy;
                }
                self.deliver_value(def, id);
                self.request_redraw();
            }
            _ => {}
        }
    }

    /// Pointer release: a held button emits 0, then the drag ends.
    fn on_release(&mut self) {
        if let (Some(def), Some(Drag::Button { id })) = (self.current_def, &self.drag) {
            let id = *id;
            self.deliver(def, id, OscType::Int(0));
            self.request_redraw();
        }
        self.drag = None;
    }

    /// Delivers a widget's current event value (the value used for `/gui_event`
    /// or the bound forward).
    fn deliver_value(&mut self, def: i32, id: i32) {
        if let Some(value) = self
            .host
            .window_def(def)
            .and_then(|t| interact::value_of(t, id))
        {
            self.deliver(def, id, value);
        }
    }

    /// Routes a widget's value: to the audio server when bound (`/gui_bind`, the
    /// bypass path), else as a `/gui_event` queued for the page.
    fn deliver(&mut self, _def: i32, id: i32, value: OscType) {
        if self.host.forward(id, value.clone()) {
            return; // bound: it went straight to the --ws audio server
        }
        self.queue(OscMessage {
            addr: GUI_EVENT.into(),
            args: vec![OscType::Int(id), value],
        });
    }
}

impl ApplicationHandler<WebEvent> for WebApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("clausters-gui (web)")
            .with_inner_size(LogicalSize::new(CANVAS_SIZE.0 as f64, CANVAS_SIZE.1 as f64))
            .with_append(true);
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => return log(&format!("cannot create the canvas window: {e}")),
        };
        self.window = Some(window.clone());
        log("opened window over <canvas>; requesting WebGPU adapter...");
        // The proxy lives in the closure; rebuild it from the event loop.
        let proxy = WEB_PROXY.with(|p| p.borrow().clone());
        wasm_bindgen_futures::spawn_local(async move {
            match Gpu::new(window).await {
                Ok(gpu) => {
                    if let Some(proxy) = proxy {
                        let _ = proxy.send_event(WebEvent::GpuReady(gpu));
                    }
                }
                Err(e) => {
                    // No WebGPU: surface a clear, actionable message instead of
                    // aborting; the canvas stays blank but the page survives.
                    log(&e);
                    set_status(&e);
                }
            }
        });
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: WebEvent) {
        match event {
            WebEvent::GpuReady(gpu) => {
                let painter = Painter::new(&gpu.device, gpu.config.format);
                self.render = Some(WindowRender {
                    gpu,
                    painter,
                    waveforms: HashMap::new(),
                });
                log("WebGPU ready");
                if self.current_def.is_some() {
                    self.build_resources();
                }
                self.request_redraw();
            }
            WebEvent::Inbound(bytes) => self.on_inbound(&bytes),
            WebEvent::ConnectServer(url) => match WsServerLink::connect(&url) {
                Ok(link) => {
                    self.host.set_server_link(ServerLink::Ws(link));
                    log(&format!("audio-server leg connecting to {url}"));
                }
                Err(e) => log(&format!("cannot open audio-server WebSocket {url}: {e}")),
            },
        }
    }

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::Resized(size) => {
                if let Some(render) = self.render.as_mut() {
                    render.gpu.resize(size.width, size.height);
                }
                self.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                if self.drag.is_some() {
                    self.on_move();
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => match state {
                ElementState::Pressed => self.on_press(),
                ElementState::Released => self.on_release(),
            },
            WindowEvent::RedrawRequested => self.draw(),
            _ => {}
        }
    }
}

thread_local! {
    /// The running app's event-loop proxy, so `resumed` can reach it for the
    /// async GPU hand-off (winit's web loop is single-threaded).
    static WEB_PROXY: RefCell<Option<EventLoopProxy<WebEvent>>> = const { RefCell::new(None) };
}

/// Builds the GPU slot for every inline-data `waveform` in the tree (the browser
/// bulk source until the network path lands).
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

/// The binding surface JS holds: feed OSC packets / GuiDefs in, drain events out,
/// and connect the audio-server WebSocket. It reaches the running app through the
/// event-loop proxy and shares the outbox queue.
#[wasm_bindgen]
pub struct GuiBridge {
    proxy: EventLoopProxy<WebEvent>,
    outbox: Rc<RefCell<VecDeque<Vec<u8>>>>,
}

#[wasm_bindgen]
impl GuiBridge {
    /// Feeds one raw OSC packet (e.g. a `/gui_def`/`/gui_set`/`/gui_bind`) to the
    /// host, exactly as the WS wire format delivers it (one packet per call).
    pub fn feed(&self, packet: &[u8]) {
        let _ = self.proxy.send_event(WebEvent::Inbound(packet.to_vec()));
    }

    /// Convenience: build and feed a `/gui_def <id> <json>` from a GuiDef JSON
    /// string — the same JSON the Python builders emit, so a page needs no OSC
    /// encoder of its own.
    pub fn def(&self, id: i32, json: &str) {
        let msg = OscMessage {
            addr: super::GUI_DEF.into(),
            args: vec![OscType::Int(id), OscType::String(json.to_string())],
        };
        match encode(&OscPacket::Message(msg)) {
            Ok(bytes) => self.feed(&bytes),
            Err(e) => log(&format!("cannot encode /gui_def: {e}")),
        }
    }

    /// Pops the next outbound OSC packet (`/gui_event`/`/gui_closed`/`/gui_info`)
    /// for the page to decode, or `undefined` when the queue is empty.
    pub fn poll(&self) -> Option<Vec<u8>> {
        self.outbox.borrow_mut().pop_front()
    }

    /// Attaches the host's audio-server leg to a `--ws` server `url`, so a bound
    /// widget forwards straight to it (the bypass path, in the browser).
    pub fn connect_server(&self, url: &str) {
        let _ = self
            .proxy
            .send_event(WebEvent::ConnectServer(url.to_string()));
    }
}

/// The wasm entry point: build the event loop, spawn the app on the browser's
/// animation-frame loop (returns immediately, nothing blocks the main thread),
/// and hand the page a [`GuiBridge`] to drive it.
#[wasm_bindgen]
pub fn start() -> GuiBridge {
    console_error_panic_hook::set_once();
    let event_loop = EventLoop::<WebEvent>::with_user_event()
        .build()
        .expect("build the web event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    WEB_PROXY.with(|p| *p.borrow_mut() = Some(proxy.clone()));
    let outbox = Rc::new(RefCell::new(VecDeque::new()));
    let bridge = GuiBridge {
        proxy,
        outbox: outbox.clone(),
    };
    log("clausters-gui web host starting");
    event_loop.spawn_app(WebApp::new(outbox));
    bridge
}
