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
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::platform::web::{EventLoopExtWebSys, WindowAttributesExtWebSys};
use winit::window::{Window, WindowId};

use crate::gpu::Gpu;
use crate::peaks::MultiPyramid;
use crate::spectrogram::Stft;
use crate::view::TimelineView;
use crate::waveform::WaveformData;

use super::fetch::{BufferFetches, FetchStep};
use super::frame::{self, SpectrogramSlot, WaveformSlot};
use super::gestures::{GestureCtx, GestureEffect, Gestures, TextKey};
use super::live::{self, StreamedBuses, StreamedTaps};
use super::paint::Painter;
use super::pianoroll;
use super::spectrum::SpectrumState;
use super::widget::{Widget, WidgetKind};
use super::{BusSource, ClientId, GUI_EVENT, Host, HostEffect, ServerLink};

/// The default canvas size; a fed GuiDef lays out into it (the layout uses the
/// framebuffer size, not the def's declared `w`/`h`).
const CANVAS_SIZE: (u32, u32) = (480, 420);

/// Logs a line to the browser console (a full `tracing` wasm shim is out of scope).
fn log(msg: &str) {
    web_sys::console::log_1(&msg.into());
}

/// Writes a status line into the page's `#note` element (if present), so a
/// failure the user must act on (no GPU adapter at all) is visible without the
/// console.
fn set_status(msg: &str) {
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("note"))
    {
        el.set_text_content(Some(msg));
    }
}

/// The host's audio-server leg over a browser `WebSocket` to a `--ws` server.
/// Bidirectional: outbound frames carry bound-widget values and the host's own
/// requests (`/bus_stream`, `/buffer_query`, `/buffer_getRange`); inbound frames (the server's
/// replies and streamed `/bus_set` snapshots) are forwarded into the event loop
/// as [`WebEvent::ServerInbound`] and decode through the one `decode_packet`
/// door. Frames sent before the socket opens are buffered and flushed on open,
/// so a `connect` immediately followed by interaction does not drop values.
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
        // Inbound: each binary frame is one OSC packet from the audio server
        // (a streamed `/bus_set`, a `/buffer_query.reply`/`/buffer_getRange.reply` reply, a `/fail`),
        // forwarded to the app through the event-loop proxy.
        let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(
            move |event: web_sys::MessageEvent| {
                let Ok(buffer) = event.data().dyn_into::<js_sys::ArrayBuffer>() else {
                    return; // non-binary frames carry nothing of ours
                };
                let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
                if let Some(proxy) = WEB_PROXY.with(|p| p.borrow().clone()) {
                    let _ = proxy.send_event(WebEvent::ServerInbound(bytes));
                }
            },
        );
        socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        on_message.forget();
        Ok(Self {
            socket,
            open,
            pending,
        })
    }

    /// Sends one OSC message as a binary frame (one packet per frame, the
    /// WebSocket wire format the audio server speaks), buffering until the
    /// socket is open.
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

/// The host's audio-server leg to the **in-page engine** (the AudioWorklet
/// backend): outbound OSC packets are handed to a page-registered JS callback
/// (which forwards them to the worklet's MessagePort); inbound replies arrive
/// through [`GuiBridge::server_reply`]. The whole audio server lives in the
/// same tab — no process, no socket, no headers.
pub struct PageServerLink {
    callback: js_sys::Function,
}

impl PageServerLink {
    /// Sends one OSC message by invoking the page's callback with the encoded
    /// bytes (a fresh `Uint8Array` per call; the page may transfer it onward).
    pub fn send(&self, msg: OscMessage) -> std::io::Result<()> {
        let bytes = encode(&OscPacket::Message(msg))
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        let array = js_sys::Uint8Array::from(bytes.as_slice());
        self.callback
            .call1(&JsValue::NULL, &array)
            .map_err(|e| std::io::Error::other(format!("{e:?}")))?;
        Ok(())
    }
}

/// What the binding surface and the async GPU hand the running app, through the
/// winit web event loop's proxy (single-threaded, so the non-`Send` `Gpu` and
/// the byte buffers move freely).
enum WebEvent {
    /// Give this def a canvas of its own. `canvas` is the element the page
    /// created for it; `None` asks winit to append one to `<body>`, which is
    /// what a page that never attaches anything gets.
    Attach {
        def_id: i32,
        canvas: Option<web_sys::HtmlCanvasElement>,
    },
    /// Drop this def's canvas: its surface, its GPU slots and its live state go.
    Detach(i32),
    /// The element's box changed size, in device pixels, with the ratio that
    /// produced them (an `ResizeObserver` box and `devicePixelRatio`): the page
    /// reports both, because the product alone cannot be undone and the host
    /// needs the ratio to resolve its logical sizes.
    Resize {
        def_id: i32,
        width: u32,
        height: u32,
        scale: f32,
    },
    /// The element entered or left the viewport (`IntersectionObserver`).
    SetVisible { def_id: i32, visible: bool },
    /// The async WebGPU device for one canvas is ready.
    GpuReady { def_id: i32, gpu: Gpu },
    /// One inbound OSC packet from the in-page binding surface (a `/gui_*`).
    Inbound(Vec<u8>),
    /// Attach the audio-server leg to this `--ws` URL (for bound widgets).
    ConnectServer(String),
    /// Attach the audio-server leg to the in-page engine: outbound packets go
    /// to this page-registered callback (replies via `server_reply`).
    ConnectPage(js_sys::Function),
    /// One inbound OSC packet from the audio server over the WS leg (a streamed
    /// `/bus_set`, a `/buffer_query.reply`/`/buffer_getRange.reply` reply, a `/fail`).
    ServerInbound(Vec<u8>),
    /// The animation tick (a `setInterval` at ~30 fps while the window has live
    /// widgets): advance the scope histories and repaint.
    Tick,
    /// A `fetch` of a waveform/plot URL completed and decoded (the browser's
    /// bulk path: `path`/`cache` resolve against the page origin).
    BulkReady {
        def_id: i32,
        widget_id: i32,
        data: BulkData,
    },
    /// A theme overlay from the page: role -> "#rrggbb[aa]" pairs (the same
    /// table `[gui.theme]` and a `--theme` file carry natively).
    Theme(Vec<(String, String)>),
    /// A metrics overlay from the page: role -> number pairs (the same table
    /// `[gui.metrics]` carries natively, `scale` included).
    Metrics(Vec<(String, f64)>),
}

/// A fetched-and-decoded bulk resource, ready to place. The decode (pyramid
/// mapping, raw-`f32` de-interleave, in-wasm pyramid/STFT build) happens in
/// the async fetch task; placing a waveform/spectrogram needs the GPU, a plot
/// only the tree.
enum BulkData {
    Waveform(WaveformData),
    Spectrogram(Vec<Stft>),
    Plot(Arc<[f32]>),
}

/// One waveform/spectrogram/plot URL to fetch and how to decode its bytes.
enum BulkRequest {
    /// A prebuilt peak-pyramid cache (mono v1 or multichannel v2), mapped
    /// straight to a [`MultiPyramid`].
    Cache(String),
    /// Raw little-endian `f32`: de-interleave every channel, build the
    /// pyramids in wasm (the analysis lives in `clausters-core`, FFI-free).
    Raw {
        url: String,
        channels: usize,
        base_bucket: usize,
    },
    /// A prebuilt (single-channel) STFT cache for a `spectrogram`.
    StftCache(String),
    /// Raw little-endian `f32` for a `spectrogram`: de-interleave every
    /// channel and analyze each in wasm.
    StftRaw {
        url: String,
        channels: usize,
        window_size: usize,
        hop: usize,
        sample_rate: f64,
    },
    /// Raw little-endian `f32` for a `plot` (kept interleaved, no pyramid).
    Plot { url: String, channels: usize },
}

/// The per-canvas GPU resources.
struct WindowRender {
    gpu: Gpu,
    painter: Painter,
    /// The editor-chrome overlay pass (selection, playhead, rulers, readout).
    overlay: Painter,
    waveforms: HashMap<i32, WaveformSlot>,
    spectrograms: HashMap<i32, SpectrogramSlot>,
}

/// One canvas: a `window`-rooted GuiDef's drawing surface and everything that
/// follows it. The browser twin of the native front's `WindowState` — the
/// desktop already keeps one of these per window-rooted def, and a document
/// holds N canvases for the same reason a desktop holds N windows.
///
/// The host learns nothing about HTML from it: the page says *this def draws
/// into this canvas, at this size, and right now it is (not) visible*.
struct CanvasSlot {
    window: Arc<Window>,
    /// The GPU resources, once the async device resolved.
    render: Option<WindowRender>,
    /// A size that arrived before the GPU was ready (so `render` was `None` and
    /// it could not be applied yet); replayed on `GpuReady` so the surface is
    /// configured to the real size for the first frame, not a stale 1x1.
    pending_size: Option<(u32, u32)>,
    /// Whether the canvas is in the viewport. A hidden one is skipped on the
    /// tick and its buses leave the subscription: a document can hold fifty
    /// canvases with three in view, and the browser's own compositing skip does
    /// not stop *us* from computing a frame or the server from streaming for it.
    visible: bool,
    cursor: (f64, f64),
    /// The finger currently driving this canvas, if any.
    ///
    /// The gesture machine is single-pointer — one press, one drag, one release
    /// — so the **first** touch owns the gesture and the rest are ignored until
    /// it lifts. A second finger landing mid-drag would otherwise teleport the
    /// value being dragged.
    touch: Option<u64>,
    /// This canvas' gesture state — the shared machine both fronts drive.
    gestures: Gestures,
    /// Modifier keys (winit `ModifiersChanged`), snapshotted into each
    /// [`GestureCtx`] so Shift-pan/Ctrl-edit/Alt-select work as on the desktop.
    shift: bool,
    ctrl: bool,
    alt: bool,
    /// Recent control-bus samples per `scope` widget id (oldest .. newest),
    /// advanced on [`WebEvent::Tick`] exactly as the native tick does.
    scopes: HashMap<i32, VecDeque<f32>>,
    /// Triggered display window per audio-rate scope widget id, refreshed on
    /// the tick (`live::update_tap_windows`). Also holds each phasescope's
    /// interleaved L/R window (ids do not collide).
    tap_windows: HashMap<i32, live::TapWindow>,
    /// Persistent FFT analysis state per `spectrum` widget id, advanced on the
    /// tick (`live::update_spectra`), exactly as the native front does.
    spectra: HashMap<i32, Vec<SpectrumState>>,
    /// Fetched waveforms/spectrograms that arrived before the GPU was ready,
    /// placed on `GpuReady` (plots need no GPU and are placed immediately).
    pending_bulk: Vec<(i32, BulkData)>,
}

impl CanvasSlot {
    fn new(window: Arc<Window>) -> Self {
        Self {
            window,
            render: None,
            pending_size: None,
            visible: true,
            cursor: (0.0, 0.0),
            touch: None,
            gestures: Gestures::default(),
            shift: false,
            ctrl: false,
            alt: false,
            scopes: HashMap::new(),
            tap_windows: HashMap::new(),
            spectra: HashMap::new(),
            pending_bulk: Vec::new(),
        }
    }

    /// Forgets everything derived from a def's tree, keeping the canvas itself
    /// — the rebuild semantics of a re-`/gui_def` and of a `/gui_free`.
    fn clear_def_state(&mut self) {
        self.scopes.clear();
        self.tap_windows.clear();
        self.spectra.clear();
        self.pending_bulk.clear();
        if let Some(render) = self.render.as_mut() {
            render.waveforms.clear();
            render.spectrograms.clear();
        }
    }

    fn fb(&self) -> (u32, u32) {
        self.render
            .as_ref()
            .map(|r| (r.gpu.config.width.max(1), r.gpu.config.height.max(1)))
            .unwrap_or((1, 1))
    }

    fn request_redraw(&self) {
        self.window.request_redraw();
    }
}

/// The browser host application: the live [`Host`], one [`CanvasSlot`] per
/// `window`-rooted def, and the shared outbox the binding surface drains.
struct WebApp {
    host: Host,
    outbox: Rc<RefCell<VecDeque<Vec<u8>>>>,
    /// One canvas per `window`-rooted def, keyed by its def id.
    canvases: HashMap<i32, CanvasSlot>,
    /// The reverse index winit's per-window events route through.
    by_winit: HashMap<WindowId, i32>,
    /// Whether the event loop resumed — a window can only be created after it,
    /// so an `attach` that arrives first waits here.
    resumed: bool,
    pending_attach: Vec<(i32, Option<web_sys::HtmlCanvasElement>)>,
    /// The piano-roll note clipboard (Ctrl+C/X/V), page-wide.
    clipboard: Vec<pianoroll::Note>,
    /// The `text` field clipboard (Ctrl+C/X/V), page-wide. An in-page clipboard
    /// like the native front's; binding it to the browser's OS clipboard (a
    /// `writeText` out plus a `paste`-event listener in) is a later refinement.
    text_clipboard: String,
    /// Live control-bus values streamed from the audio server (`/bus_stream` →
    /// `/bus_set`), the browser's [`BusSource`] for meters/scopes/canvases.
    buses: Arc<StreamedBuses>,
    /// The bus set currently subscribed with `/bus_stream` (sorted), so a tree
    /// change only resubscribes when the set actually changed.
    streamed: Vec<i32>,
    /// The newest `/bus_tapStream.reply` window per tap — the browser's source for
    /// audio-rate scopes, read on the tick exactly as the native front reads
    /// the segment's tap rings.
    taps: Arc<StreamedTaps>,
    /// The `(taps, window frames)` currently subscribed with `/bus_tapStream`,
    /// so a tree change only resubscribes when they actually changed.
    tap_streamed: (Vec<i32>, usize),
    /// The server's sample rate (from `/clock_query.reply`, requested when the leg
    /// connects); `0.0` until known — window sizing then assumes 48 kHz.
    server_rate: f64,
    /// The engine's sample clock from the newest `/clock_query.reply` — the browser
    /// playhead source (polled once per tick while a playhead is shown; the
    /// native front reads the shm header instead).
    server_clock: f64,
    /// The animation tick: the `setInterval` id and its closure, kept alive
    /// while the current def has live widgets (meter/scope/canvas).
    tick: Option<(i32, Closure<dyn FnMut()>)>,
    /// Whether the first streamed `/bus_set` snapshot was logged (one line as
    /// evidence the bus stream is flowing; logging every frame would spam).
    stream_seen: bool,
    /// The server-buffer fetch machine (`/buffer_query` → chunked `/buffer_getRange`),
    /// shared with the native front; requests ride the WS leg.
    fetches: BufferFetches,
}

impl WebApp {
    fn new(outbox: Rc<RefCell<VecDeque<Vec<u8>>>>) -> Self {
        Self {
            host: Host::new(),
            outbox,
            canvases: HashMap::new(),
            by_winit: HashMap::new(),
            resumed: false,
            pending_attach: Vec::new(),
            clipboard: Vec::new(),
            text_clipboard: String::new(),
            buses: Arc::new(StreamedBuses::default()),
            streamed: Vec::new(),
            taps: Arc::new(StreamedTaps::default()),
            tap_streamed: (Vec::new(), 0),
            server_rate: 0.0,
            server_clock: 0.0,
            tick: None,
            stream_seen: false,
            fetches: BufferFetches::default(),
        }
    }

    /// Gives `def_id` a canvas and starts its GPU bring-up.
    ///
    /// `canvas` is the element the component created — the correct ownership,
    /// and the only way N of them can exist. `None` keeps the older posture, a
    /// canvas winit appends to `<body>`, which is what a page that feeds a
    /// `/gui_def` without attaching anything gets.
    fn attach(
        &mut self,
        event_loop: &ActiveEventLoop,
        def_id: i32,
        canvas: Option<web_sys::HtmlCanvasElement>,
    ) {
        if !self.resumed {
            // A window cannot be created before the loop resumes; `resumed`
            // drains this.
            self.pending_attach.push((def_id, canvas));
            return;
        }
        if let Some(old) = self.canvases.remove(&def_id) {
            self.by_winit.remove(&old.window.id());
        }
        let appending = canvas.is_none();
        let attrs = Window::default_attributes()
            .with_title(format!("clausters-gui {def_id}"))
            .with_inner_size(LogicalSize::new(CANVAS_SIZE.0 as f64, CANVAS_SIZE.1 as f64))
            // Not focused on creation: winit focuses a new canvas, and a
            // browser scrolls a freshly focused element into view — so in a
            // document with several components the last one mounted would yank
            // the reader down to it. A click focuses it, which is when keyboard
            // input is wanted anyway.
            .with_active(false)
            .with_canvas(canvas)
            .with_append(appending);
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => return log(&format!("def {def_id}: cannot open a canvas: {e}")),
        };
        self.by_winit.insert(window.id(), def_id);
        self.canvases
            .insert(def_id, CanvasSlot::new(window.clone()));
        log(&format!(
            "def {def_id}: canvas attached; requesting GPU adapter"
        ));
        let proxy = WEB_PROXY.with(|p| p.borrow().clone());
        wasm_bindgen_futures::spawn_local(async move {
            match Gpu::new(window).await {
                Ok(gpu) => {
                    if let Some(proxy) = proxy {
                        let _ = proxy.send_event(WebEvent::GpuReady { def_id, gpu });
                    }
                }
                Err(e) => {
                    // No GPU adapter at all (neither WebGPU nor WebGL2): surface
                    // a clear, actionable message instead of aborting; the canvas
                    // stays blank but the page survives.
                    log(&e);
                    set_status(&e);
                }
            }
        });
    }

    /// Drops a def's canvas: the wgpu surface and every derived resource go.
    /// The `<canvas>` element itself belongs to the page, which removes it.
    fn detach(&mut self, def_id: i32) {
        if let Some(slot) = self.canvases.remove(&def_id) {
            self.by_winit.remove(&slot.window.id());
        }
        self.pending_attach.retain(|(id, _)| *id != def_id);
        self.fetches.drop_def(def_id);
        self.on_tree_changed();
    }

    /// Handles one inbound OSC packet (from the binding surface) through the real
    /// protocol dispatch, then carries out the effects: open/redraw the window,
    /// queue replies for the page to drain.
    fn on_inbound(&mut self, event_loop: &ActiveEventLoop, bytes: &[u8]) {
        let packet = match decode_packet(bytes) {
            Ok(p) => p,
            Err(e) => return log(&format!("malformed OSC packet from the page: {e}")),
        };
        for effect in self.host.handle_packet(packet, ClientId::Web) {
            match effect {
                HostEffect::Reply(msg) => self.queue(msg),
                HostEffect::OpenWindow(id) => {
                    log(&format!("/gui_def {id}: opened from the page"));
                    // A page that fed a `/gui_def` without attaching a canvas
                    // gets one appended, as the single-canvas host always did.
                    if !self.canvases.contains_key(&id) {
                        self.attach(event_loop, id, None);
                    }
                    if let Some(slot) = self.canvases.get_mut(&id) {
                        slot.clear_def_state();
                    }
                    self.fetches.drop_def(id); // rebuild semantics on a re-/gui_def
                    self.build_resources(id);
                    self.request_redraw(id);
                    self.start_bulk(id);
                    self.on_tree_changed();
                }
                HostEffect::CloseWindow(id) => {
                    if let Some(slot) = self.canvases.get_mut(&id) {
                        slot.clear_def_state();
                        slot.request_redraw();
                    }
                    self.fetches.drop_def(id);
                    self.on_tree_changed();
                }
                HostEffect::Redraw(id) => {
                    if self.canvases.contains_key(&id) {
                        // A `/gui_set` may have retargeted a meter/scope `bus`:
                        // re-derive the subscription (a no-op when unchanged).
                        self.on_tree_changed();
                        self.request_redraw(id);
                    }
                }
            }
        }
    }

    /// The defs currently drawing: one canvas each, and in the viewport.
    ///
    /// Everything that costs something per frame or per packet — the tick, the
    /// `/bus_stream` and `/bus_tapStream` subscriptions — is derived from exactly
    /// this set, which is what makes a scrolled-away component free.
    fn visible_defs(&self) -> Vec<i32> {
        let mut ids: Vec<i32> = self
            .canvases
            .iter()
            .filter(|(_, slot)| slot.visible)
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        ids
    }

    /// A freshly attached server leg (WS or in-page) holds no subscription:
    /// forget the old ones and subscribe the current tree's buses and taps
    /// (WS frames queue until the socket opens, so sending now is safe).
    /// `/clock_query` fetches the rate the oscilloscope windows are sized with.
    fn on_server_attached(&mut self) {
        self.streamed.clear();
        self.tap_streamed = (Vec::new(), 0);
        if let Some(server) = self.host.server() {
            let _ = server.send(OscMessage {
                addr: "/clock_query".into(),
                args: vec![],
            });
        }
        self.on_tree_changed();
    }

    /// Re-derives everything that follows from the current tree's live widgets:
    /// the `/bus_stream` and `/bus_tapStream` subscriptions on the WS leg and the
    /// animation tick. Called on open/close/redraw and after the server leg
    /// attaches; cheap (a tree walk) and idempotent, so calling it eagerly is
    /// fine.
    fn on_tree_changed(&mut self) {
        let demand = self.demand();
        self.sync_bus_stream(demand.buses);
        self.sync_tap_stream(demand.taps, demand.tap_frames);
        self.ensure_tick(demand.animated);
    }

    /// What the drawing canvases ask of the server and of the frame clock —
    /// the union over the **visible** set, so a scrolled-away component drops
    /// out of it. The derivation itself is platform-agnostic
    /// ([`live::demand`]), natively tested.
    fn demand(&self) -> live::LiveDemand {
        let trees: Vec<&Widget> = self
            .visible_defs()
            .into_iter()
            .filter_map(|def| self.host.window_def(def))
            .collect();
        live::demand(trees, self.server_rate)
    }

    /// Subscribes the audio server to exactly the control buses the drawing
    /// canvases read live (`/bus_stream`, replacing this client's previous
    /// subscription), or cancels when none are left. Skipped without a server
    /// leg; `ConnectServer` re-runs it once the leg exists.
    fn sync_bus_stream(&mut self, wanted: Vec<i32>) {
        if wanted == self.streamed {
            return;
        }
        let Some(server) = self.host.server() else {
            return;
        };
        let mut args = Vec::with_capacity(wanted.len() + 1);
        if wanted.is_empty() {
            args.push(OscType::Int(0)); // periodMs 0: cancel
        } else {
            args.push(OscType::Int(live::STREAM_PERIOD_MS));
            args.extend(wanted.iter().map(|&bus| OscType::Int(bus)));
        }
        match server.send(OscMessage {
            addr: "/bus_stream".into(),
            args,
        }) {
            Ok(()) => {
                log(&format!("/bus_stream subscription: {wanted:?}"));
                self.streamed = wanted;
            }
            Err(e) => log(&format!("failed to (re)subscribe /bus_stream: {e}")),
        }
    }

    /// Subscribes the audio server to exactly the audio taps the drawing
    /// canvases' oscilloscopes read (`/bus_tapStream`, replacing this client's
    /// previous subscription), sized to the largest raw window any of them
    /// needs; cancels when none are left. Skipped without a server leg.
    fn sync_tap_stream(&mut self, wanted: Vec<i32>, frames: usize) {
        if (wanted.clone(), frames) == self.tap_streamed {
            return;
        }
        let Some(server) = self.host.server() else {
            return;
        };
        let mut args = Vec::with_capacity(wanted.len() + 2);
        if wanted.is_empty() {
            args.push(OscType::Int(0)); // periodMs 0: cancel
            args.push(OscType::Int(0));
        } else {
            args.push(OscType::Int(live::STREAM_PERIOD_MS));
            args.push(OscType::Int(frames as i32));
            args.extend(wanted.iter().map(|&tap| OscType::Int(tap)));
        }
        match server.send(OscMessage {
            addr: "/bus_tapStream".into(),
            args,
        }) {
            Ok(()) => {
                log(&format!(
                    "/bus_tapStream subscription: {wanted:?} x{frames}"
                ));
                self.tap_streamed = (wanted, frames);
            }
            Err(e) => log(&format!("failed to (re)subscribe /bus_tapStream: {e}")),
        }
    }

    /// Starts or stops the ~30 fps animation tick to match the drawing
    /// canvases: running while any has live widgets (meter/scope/canvas),
    /// stopped otherwise. The tick advances the scope histories and repaints —
    /// the browser twin of the native `about_to_wait` frame timer, driven by
    /// `setInterval` because `std::time::Instant` does not exist on wasm.
    fn ensure_tick(&mut self, animated: bool) {
        let Some(window) = web_sys::window() else {
            return;
        };
        match (animated, self.tick.is_some()) {
            (true, false) => {
                let closure = Closure::<dyn FnMut()>::new(move || {
                    if let Some(proxy) = WEB_PROXY.with(|p| p.borrow().clone()) {
                        let _ = proxy.send_event(WebEvent::Tick);
                    }
                });
                match window.set_interval_with_callback_and_timeout_and_arguments_0(
                    closure.as_ref().unchecked_ref(),
                    live::STREAM_PERIOD_MS,
                ) {
                    Ok(id) => self.tick = Some((id, closure)),
                    Err(e) => log(&format!("cannot start the animation tick: {e:?}")),
                }
            }
            (false, true) => {
                if let Some((id, _closure)) = self.tick.take() {
                    window.clear_interval_with_handle(id);
                }
            }
            _ => {}
        }
    }

    /// One animation tick: push a fresh streamed-bus sample into every scope's
    /// rolling history and refresh the audio-rate scopes' triggered windows
    /// from the `/bus_tapStream.reply` store (time-based, exactly like the native tick),
    /// then repaint.
    fn on_tick(&mut self) {
        let mut wants_clock = false;
        for def in self.visible_defs() {
            let Some(tree) = self.host.window_def(def) else {
                continue;
            };
            let Some(slot) = self.canvases.get_mut(&def) else {
                continue;
            };
            let buses = &self.buses;
            live::advance_scope_histories(
                tree,
                |bus| {
                    if bus < 0 {
                        0.0
                    } else {
                        buses.control(bus as usize)
                    }
                },
                &mut slot.scopes,
            );
            let taps = &self.taps;
            live::update_tap_windows(
                tree,
                self.server_rate,
                |tap, out| taps.read_raw(tap, out),
                &mut slot.tap_windows,
            );
            live::update_phase_windows(
                tree,
                self.server_rate,
                |tap, out| taps.read_raw(tap, out),
                &mut slot.tap_windows,
            );
            live::update_spectra(tree, |tap, out| taps.read_raw(tap, out), &mut slot.spectra);
            wants_clock |= live::tree_has_playhead(tree);
            slot.request_redraw();
        }
        // A visible playhead needs the engine clock: poll it once per tick (the
        // browser's stand-in for the shm header's sample clock) — once for the
        // page, however many canvases show one.
        if wants_clock && let Some(server) = self.host.server() {
            let _ = server.send(OscMessage {
                addr: "/clock_query".into(),
                args: vec![],
            });
        }
        self.advance_edge_scroll();
    }

    /// The frame step of a clip drag held against a lane's edge: pans the view
    /// and carries the clip with it. The tick's own period is the `dt`, so the
    /// scroll runs at the same speed whatever else the frame is doing.
    fn advance_edge_scroll(&mut self) {
        let dt = f64::from(live::STREAM_PERIOD_MS) / 1000.0;
        let dragging: Vec<i32> = self
            .canvases
            .iter()
            .filter(|(_, slot)| slot.gestures.edge_scrolling(slot.cursor.0))
            .map(|(def, _)| *def)
            .collect();
        for def in dragging {
            let Some((ctx, (cx, _cy))) = self.gesture_ctx(def) else {
                continue;
            };
            let Some(slot) = self.canvases.get_mut(&def) else {
                continue;
            };
            let effects = slot.gestures.tick(&mut self.host, &ctx, cx, dt);
            self.apply_gesture_effects(effects);
        }
    }

    /// Routes one decoded OSC packet from the audio server (the WS leg): the
    /// streamed `/bus_set` snapshots into [`StreamedBuses`], the buffer replies
    /// into the shared fetch machine. The browser twin of the native
    /// `handle_server_packet`.
    fn on_server_inbound(&mut self, bytes: &[u8]) {
        let packet = match decode_packet(bytes) {
            Ok(p) => p,
            Err(e) => return log(&format!("malformed OSC packet from the server: {e}")),
        };
        let OscPacket::Message(msg) = packet else {
            return; // bundles are not used on the reply path
        };
        match msg.addr.as_str() {
            "/bus_stream.reply" => {
                if !self.stream_seen {
                    self.stream_seen = true;
                    log(&format!("bus stream flowing: {:?}", msg.args));
                }
                for pair in msg.args.chunks(2) {
                    if let [OscType::Int(index), OscType::Float(value)] = pair
                        && *index >= 0
                    {
                        self.buses.set(*index as usize, *value);
                    }
                }
            }
            "/buffer_query.reply" => {
                // (bufnum, frames, channels, sampleRate) per buffer.
                for group in msg.args.chunks(4) {
                    if let [
                        OscType::Int(bufnum),
                        OscType::Int(frames),
                        OscType::Int(channels),
                        rate,
                    ] = group
                    {
                        let rate = match rate {
                            OscType::Float(x) => *x as f64,
                            OscType::Double(x) => *x,
                            _ => 0.0,
                        };
                        let step = self.fetches.on_info(
                            *bufnum,
                            (*frames).max(0) as usize,
                            (*channels).max(0) as usize,
                            rate,
                        );
                        self.apply_fetch_step(step);
                    }
                }
            }
            "/buffer_getRange.reply" => {
                let step = self.fetches.on_data(&msg.args);
                self.apply_fetch_step(step);
            }
            "/bus_tapStream.reply" => {
                // (tap, stream position, raw LE f32 blob): the newest window
                // of one tap; store it for the tick to align and draw.
                if let (Some(OscType::Int(tap)), Some(OscType::Blob(bytes))) =
                    (msg.args.first(), msg.args.get(2))
                {
                    let samples: Vec<f32> = bytes
                        .chunks_exact(4)
                        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                        .collect();
                    self.taps.set(*tap, samples);
                }
            }
            "/clock_query.reply" => {
                // (samples, rate, ...): keep the rate for window sizing and
                // the sample counter for the timeline playhead.
                if let Some(OscType::Long(samples)) = msg.args.first() {
                    self.server_clock = *samples as f64;
                }
                if let Some(OscType::Double(rate)) = msg.args.get(1) {
                    self.server_rate = *rate;
                    // Window sizes may change with the real rate known.
                    let demand = self.demand();
                    self.sync_tap_stream(demand.taps, demand.tap_frames);
                }
            }
            "/fail" => log(&format!("audio server replied /fail: {:?}", msg.args)),
            // `/done` acks (e.g. for `/bus_stream`) need no action.
            "/done" => {}
            _ => {}
        }
    }

    /// Carries out one fetch-machine step: send the next request over the WS
    /// leg, or turn a finished buffer into view data for its widgets —
    /// looking each widget up in the tree, like the native front, to decide
    /// between a multichannel waveform and per-channel STFT lanes.
    fn apply_fetch_step(&mut self, step: FetchStep) {
        match step {
            FetchStep::Request(msg) => self.send_to_server(msg),
            FetchStep::Done {
                bufnum,
                samples,
                channels,
                sample_rate,
                wants,
            } => {
                let channels = channels.max(1);
                log(&format!(
                    "buffer {bufnum}: {} frames x {channels} channel(s) loaded into {} view(s)",
                    samples.len() / channels,
                    wants.len()
                ));
                for want in wants {
                    if !self.canvases.contains_key(&want.def_id) {
                        continue;
                    }
                    let Some(kind) = self
                        .host
                        .window_def(want.def_id)
                        .and_then(|t| t.find(want.widget_id))
                        .map(|w| w.kind.clone())
                    else {
                        continue;
                    };
                    match kind {
                        WidgetKind::Waveform { base_bucket, .. } => {
                            let data =
                                WaveformData::from_interleaved(&samples, channels, base_bucket);
                            self.place_bulk(want.def_id, want.widget_id, BulkData::Waveform(data));
                        }
                        WidgetKind::Clip { base_bucket, .. } => {
                            // A clip's take lands in the tree (its lane body is
                            // flat geometry decimated from the pyramid, no GPU
                            // slot) — the same landing the mapped bulk path uses.
                            let data =
                                WaveformData::from_interleaved(&samples, channels, base_bucket);
                            self.set_clip_body(want.def_id, want.widget_id, data);
                            // Falls through to the shared tail: a clip carries
                            // no editor props, so the sample-rate fill is a
                            // no-op for it, but the **repaint** is not — a
                            // `continue` here left the take sitting in the tree
                            // with the canvas still showing the frame before it.
                        }
                        WidgetKind::Spectrogram {
                            window_size,
                            hop,
                            sample_rate: rate_prop,
                            ..
                        } => {
                            let rate = if rate_prop > 0.0 {
                                rate_prop
                            } else {
                                sample_rate
                            };
                            let stfts = frame::stft_lanes(
                                frame::deinterleave(&samples, channels),
                                window_size,
                                hop,
                                rate,
                            );
                            self.place_bulk(
                                want.def_id,
                                want.widget_id,
                                BulkData::Spectrogram(stfts),
                            );
                        }
                        _ => continue,
                    }
                    // Let the ruler label real time when the widget knew no rate.
                    if sample_rate > 0.0
                        && let Some(w) = self
                            .host
                            .window_def_mut(want.def_id)
                            .and_then(|t| t.find_mut(want.widget_id))
                        && let Some(editor) = w.kind.editor_mut()
                        && editor.sample_rate <= 0.0
                    {
                        editor.sample_rate = sample_rate;
                    }
                    self.request_redraw(want.def_id);
                }
            }
            FetchStep::None => {}
        }
    }

    /// Sends one fetch-machine message over the WS leg (`/buffer_query`, `/buffer_getRange`),
    /// logging instead of failing when no server is attached.
    fn send_to_server(&self, msg: OscMessage) {
        let Some(server) = self.host.server() else {
            return log("waveform references a server buffer but no --ws server is connected");
        };
        let addr = msg.addr.clone();
        if let Err(e) = server.send(msg) {
            log(&format!("failed to send {addr} to the audio server: {e}"));
        }
    }

    /// Starts the bulk loads of a freshly opened def: server-buffer fetches
    /// over the WS leg, and `fetch`es of every waveform/plot `path`/`cache`
    /// (URLs against the page origin in the browser).
    fn start_bulk(&mut self, def: i32) {
        let Some(tree) = self.host.window_def(def) else {
            return;
        };
        let mut buffer_refs = Vec::new();
        let mut requests = Vec::new();
        collect_bulk(tree, &mut buffer_refs, &mut requests);
        for (widget_id, bufnum) in buffer_refs {
            if let Some(query) = self.fetches.want(def, widget_id, bufnum) {
                self.send_to_server(query);
            }
        }
        for (widget_id, request) in requests {
            wasm_bindgen_futures::spawn_local(fetch_bulk(def, widget_id, request));
        }
    }

    /// Places a decoded GPU-bound resource (waveform or spectrogram) on its
    /// def's canvas: a slot right away when that device is up, else stashed and
    /// replayed on `GpuReady`.
    fn place_bulk(&mut self, def_id: i32, widget_id: i32, data: BulkData) {
        let Some(slot) = self.canvases.get_mut(&def_id) else {
            return; // the canvas was detached while the fetch was in flight
        };
        let Some(render) = slot.render.as_mut() else {
            slot.pending_bulk.push((widget_id, data));
            return;
        };
        let mut total = None;
        match data {
            BulkData::Waveform(data) => {
                let slot = frame::waveform_slot(data, &render.gpu);
                total = Some(slot.view.total_samples());
                render.waveforms.insert(widget_id, slot);
            }
            BulkData::Spectrogram(stfts) => {
                if let Some(slot) = frame::spectrogram_slot(stfts, &render.gpu) {
                    total = Some(slot.total_samples());
                    render.spectrograms.insert(widget_id, slot);
                }
            }
            BulkData::Plot(_) => unreachable!("plots are placed in the tree, not the GPU"),
        }
        // The loaded extent joins the widget's navigation group.
        if let Some(total) = total {
            self.host.set_timeline_total(widget_id, total);
        }
    }

    /// Writes a fetched take into a `clip`'s body in the host tree — the clip
    /// counterpart of a plot's samples (a lane needs no GPU slot: its body is
    /// flat geometry, decimated from the take's peak pyramid).
    fn set_clip_body(&mut self, def_id: i32, widget_id: i32, data: WaveformData) {
        if let Some(root) = self.host.window_def_mut(def_id)
            && let Some(widget) = root.find_mut(widget_id)
            && let WidgetKind::Clip { body, .. } = &mut widget.kind
        {
            *body = Some(Arc::new(data));
        }
    }

    /// A fetched bulk resource arrived: place a waveform/spectrogram (GPU
    /// slot), write a clip's take or a plot's samples into the host tree, then
    /// repaint.
    fn on_bulk_ready(&mut self, def: i32, widget_id: i32, data: BulkData) {
        // A waveform resource wanted by a `clip` lands in the tree, not the GPU.
        if let BulkData::Waveform(_) = &data
            && self
                .host
                .window_def(def)
                .and_then(|t| t.find(widget_id))
                .is_some_and(|w| matches!(w.kind, WidgetKind::Clip { .. }))
        {
            let BulkData::Waveform(data) = data else {
                unreachable!()
            };
            self.set_clip_body(def, widget_id, data);
            self.request_redraw(def);
            return;
        }
        match data {
            BulkData::Waveform(_) | BulkData::Spectrogram(_) => {
                self.place_bulk(def, widget_id, data);
            }
            BulkData::Plot(samples) => {
                if let Some(root) = self.host.window_def_mut(def)
                    && let Some(widget) = root.find_mut(widget_id)
                    && let WidgetKind::Plot {
                        samples: plot_samples,
                        ..
                    } = &mut widget.kind
                {
                    *plot_samples = samples;
                    // Landed samples feed the spectral view: refresh its cache.
                    widget.kind.refresh_plot_analysis();
                }
            }
        }
        self.request_redraw(def);
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

    /// (Re)builds one canvas' GPU resources: the inline-data waveform/
    /// spectrogram views (`path`/`cache`/`buffer` references load async through
    /// [`fetch_bulk`] and the fetch machine).
    fn build_resources(&mut self, def: i32) {
        let Some(slot) = self.canvases.get(&def) else {
            return;
        };
        let Some(render) = slot.render.as_ref() else {
            return;
        };
        let Some(tree) = self.host.window_def(def) else {
            return;
        };
        let mut waveforms = HashMap::new();
        let mut spectrograms = HashMap::new();
        build_inline_timelines(tree, &render.gpu, &mut waveforms, &mut spectrograms);
        // Each inline view's extent joins its navigation group.
        let mut totals: Vec<(i32, usize)> = Vec::new();
        totals.extend(
            waveforms
                .iter()
                .map(|(id, s)| (*id, s.view.total_samples())),
        );
        totals.extend(spectrograms.iter().map(|(id, s)| (*id, s.total_samples())));
        if let Some(render) = self.canvases.get_mut(&def).and_then(|s| s.render.as_mut()) {
            render.waveforms = waveforms;
            render.spectrograms = spectrograms;
        }
        for (id, total) in totals {
            self.host.set_timeline_total(id, total);
        }
    }

    /// Renders one canvas' def through the shared frame path. The live inputs
    /// come from the streamed buses (meters/canvases read them in `render`, the
    /// scopes their tick-fed histories); the node tree stays empty until a
    /// browser node-tree path exists.
    fn draw(&mut self, def: i32) {
        let server_attached = self.host.server().is_some();
        let focused_text = self
            .host
            .focused_text()
            .filter(|(d, _)| *d == def)
            .map(|(_, id)| id);
        let timelines = self.host.timelines();
        let theme = &self.host.theme;
        let Some(tree) = self.host.window_def(def) else {
            return;
        };
        let Some(slot) = self.canvases.get_mut(&def) else {
            return;
        };
        let inputs = frame::FrameInputs {
            metrics: self.host.metrics_for(def),
            bus: Some(self.buses.as_ref() as &dyn BusSource),
            active_button: slot.gestures.active_button(),
            focused_text,
            server_attached,
            sample_rate: self.server_rate,
            sample_clock: self.server_clock,
            cursor: Some(slot.cursor),
            timelines,
            // A rewiring drag in flight draws its wire to the pointer.
            wiring: slot
                .gestures
                .wiring()
                .map(|(id, port)| (id, port, (slot.cursor.0 as f32, slot.cursor.1 as f32))),
            marquee: slot.gestures.marquee(),
            ..Default::default()
        };
        let scopes = &slot.scopes;
        let tap_windows = &slot.tap_windows;
        let spectra = &slot.spectra;
        let Some(render) = slot.render.as_mut() else {
            return;
        };
        let mut canvases = HashMap::new();
        frame::render(
            &mut render.gpu,
            &mut render.painter,
            &mut render.overlay,
            &mut render.waveforms,
            &mut render.spectrograms,
            &mut canvases,
            scopes,
            tap_windows,
            spectra,
            tree,
            &inputs,
            theme,
        );
    }

    /// Schedules a repaint of one canvas through winit's redraw request
    /// (drawing happens in `RedrawRequested`, the idiomatic path on the
    /// browser's animation frame).
    fn request_redraw(&self, def: i32) {
        if let Some(slot) = self.canvases.get(&def) {
            slot.request_redraw();
        }
    }

    /// Snapshots the gesture context for one canvas: its framebuffer size, its
    /// modifier keys, and the heavy views' lane counts (channel/lane splits
    /// live in this front's GPU slots, so they are copied out here) — the
    /// browser twin of the native front's snapshot.
    fn gesture_ctx(&self, def: i32) -> Option<(GestureCtx, (f64, f64))> {
        let slot = self.canvases.get(&def)?;
        let (fb_w, fb_h) = slot.fb();
        let mut ctx = GestureCtx::new(def, fb_w, fb_h);
        ctx.shift = slot.shift;
        ctx.ctrl = slot.ctrl;
        ctx.alt = slot.alt;
        if let Some(render) = slot.render.as_ref() {
            for (id, view) in &render.waveforms {
                ctx.wave_lanes.insert(*id, view.view.num_channels());
            }
            for (id, view) in &render.spectrograms {
                ctx.spect_lanes.insert(*id, view.views.len());
            }
        }
        Some((ctx, slot.cursor))
    }

    /// Carries out a gesture's effects over this front's sinks: `/gui_event`s
    /// to the page outbox (a bound widget already forwarded inside the
    /// machine), and a repaint of the canvas the effect names — a linked-view
    /// mutation can name a *different* def than the one gestured on, and with a
    /// canvas each that now lands where it belongs. There is no pointer grab in
    /// the browser (the grab callback returns `false`), so releases are no-ops.
    fn apply_gesture_effects(&mut self, effects: Vec<GestureEffect>) {
        for effect in effects {
            match effect {
                GestureEffect::Emit {
                    widget_id, args, ..
                } => {
                    let mut msg_args = vec![OscType::Int(widget_id)];
                    msg_args.extend(args);
                    self.queue(OscMessage {
                        addr: GUI_EVENT.into(),
                        args: msg_args,
                    });
                }
                GestureEffect::Redraw(def_id) => self.request_redraw(def_id),
                GestureEffect::ReleasePointer(_) => {}
            }
        }
    }

    /// Pointer press: the shared gesture machine acts by widget kind.
    fn on_press(&mut self, def: i32) {
        let Some((ctx, (cx, cy))) = self.gesture_ctx(def) else {
            return;
        };
        let Some(slot) = self.canvases.get_mut(&def) else {
            return;
        };
        let effects = slot
            .gestures
            .press(&mut self.host, &ctx, cx, cy, &mut || false);
        self.apply_gesture_effects(effects);
        // A clip drag needs the frame tick even on an otherwise still window:
        // held against a lane's edge it scrolls the view, and a standing cursor
        // sends no events of its own.
        if self
            .canvases
            .get(&def)
            .is_some_and(|s| s.gestures.dragging())
        {
            self.ensure_tick(true);
        }
    }

    /// Pointer move while dragging: the machine drives the dragged target.
    fn on_move(&mut self, def: i32) {
        let Some((ctx, (cx, cy))) = self.gesture_ctx(def) else {
            return;
        };
        let Some(slot) = self.canvases.get_mut(&def) else {
            return;
        };
        let effects = slot.gestures.drag_to(&mut self.host, &ctx, cx, cy);
        self.apply_gesture_effects(effects);
    }

    /// Pointer release: the machine finishes the drag (button up, wire landing).
    fn on_release(&mut self, def: i32) {
        let Some((ctx, (cx, cy))) = self.gesture_ctx(def) else {
            return;
        };
        let Some(slot) = self.canvases.get_mut(&def) else {
            return;
        };
        let effects = slot.gestures.release(&mut self.host, &ctx, cx, cy);
        self.apply_gesture_effects(effects);
        // The drag is over: the tick goes back to what the tree actually asks
        // for (it stays on only if a live widget wants it).
        self.on_tree_changed();
    }

    /// Wheel: the machine zooms the time axis or the vertical display window.
    fn on_wheel(&mut self, def: i32, steps: f64) {
        let Some((ctx, (cx, cy))) = self.gesture_ctx(def) else {
            return;
        };
        let Some(slot) = self.canvases.get_mut(&def) else {
            return;
        };
        let effects = slot.gestures.wheel(&mut self.host, &ctx, cx, cy, steps);
        self.apply_gesture_effects(effects);
    }

    /// Keyboard: the same editing operations the desktop front maps (Delete,
    /// Ctrl+C/X/V, `q` quantize, `r` reset) — minus Escape, which closes an OS
    /// window there but has no window to close here.
    fn on_key(&mut self, def: i32, key: &Key) {
        let Some((ctx, (cx, cy))) = self.gesture_ctx(def) else {
            return;
        };
        let ctrl = ctx.ctrl;
        // A focused text field consumes the key first (typing, caret motion,
        // cut/copy/paste); only otherwise do the global shortcuts run.
        if let Some(tk) = to_text_key(key) {
            let Some(slot) = self.canvases.get_mut(&def) else {
                return;
            };
            if let Some(effects) =
                slot.gestures
                    .text_key(&mut self.host, &ctx, tk, &mut self.text_clipboard)
            {
                self.apply_gesture_effects(effects);
                return;
            }
        }
        let Some(slot) = self.canvases.get_mut(&def) else {
            return;
        };
        let effects = match key {
            Key::Named(NamedKey::Delete) | Key::Named(NamedKey::Backspace) => {
                slot.gestures.delete_selected(&mut self.host, &ctx, cx, cy)
            }
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("c") => slot
                .gestures
                .copy_selected(&mut self.host, &ctx, cx, cy, false, &mut self.clipboard),
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("x") => slot
                .gestures
                .copy_selected(&mut self.host, &ctx, cx, cy, true, &mut self.clipboard),
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("v") => slot
                .gestures
                .paste_at_cursor(&mut self.host, &ctx, cx, cy, &self.clipboard),
            Key::Character(c) if c.eq_ignore_ascii_case("q") => {
                slot.gestures.quantize(&mut self.host, &ctx, cx, cy)
            }
            Key::Character(c) if c.eq_ignore_ascii_case("r") => {
                slot.gestures.reset_timelines(&mut self.host, &ctx)
            }
            _ => return,
        };
        self.apply_gesture_effects(effects);
    }
}

/// Translates a winit key into the platform-neutral [`TextKey`] a focused
/// `text` field edits with (the browser front's twin of the native
/// `to_text_key`), or `None` for a key it does not handle.
fn to_text_key(key: &Key) -> Option<TextKey> {
    match key {
        Key::Named(NamedKey::Backspace) => Some(TextKey::Backspace),
        Key::Named(NamedKey::Delete) => Some(TextKey::Delete),
        Key::Named(NamedKey::ArrowLeft) => Some(TextKey::Left),
        Key::Named(NamedKey::ArrowRight) => Some(TextKey::Right),
        Key::Named(NamedKey::ArrowUp) => Some(TextKey::Up),
        Key::Named(NamedKey::ArrowDown) => Some(TextKey::Down),
        Key::Named(NamedKey::Home) => Some(TextKey::Home),
        Key::Named(NamedKey::End) => Some(TextKey::End),
        Key::Named(NamedKey::Enter) => Some(TextKey::Enter),
        Key::Named(NamedKey::Space) => Some(TextKey::Char(' ')),
        Key::Character(s) => s
            .chars()
            .next()
            .filter(|c| !c.is_control())
            .map(TextKey::Char),
        _ => None,
    }
}

impl ApplicationHandler<WebEvent> for WebApp {
    /// Nothing opens on its own: a canvas exists because the page attached one
    /// (or because a `/gui_def` arrived without one). This only unblocks the
    /// attaches that raced ahead of the loop.
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.resumed = true;
        for (def_id, canvas) in std::mem::take(&mut self.pending_attach) {
            self.attach(event_loop, def_id, canvas);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: WebEvent) {
        match event {
            WebEvent::Attach { def_id, canvas } => self.attach(event_loop, def_id, canvas),
            WebEvent::Detach(def_id) => self.detach(def_id),
            WebEvent::Resize {
                def_id,
                width,
                height,
                scale,
            } => {
                // The page's device-pixel ratio is this canvas' UI scale: the
                // wire's logical sizes resolve against it, once per change.
                self.host.set_ui_scale(def_id, scale);
                let Some(slot) = self.canvases.get_mut(&def_id) else {
                    return;
                };
                // The element owns the box; the host is only told the pixels.
                // winit writes the canvas' backing size, and the surface follows
                // right here rather than waiting for the `Resized` echo.
                let _ = slot
                    .window
                    .request_inner_size(PhysicalSize::new(width, height));
                match slot.render.as_mut() {
                    Some(render) => render.gpu.resize(width, height),
                    None => slot.pending_size = Some((width, height)),
                }
                slot.request_redraw();
            }
            WebEvent::SetVisible { def_id, visible } => {
                let Some(slot) = self.canvases.get_mut(&def_id) else {
                    return;
                };
                if slot.visible == visible {
                    return;
                }
                slot.visible = visible;
                if visible {
                    slot.request_redraw();
                }
                // The subscriptions and the tick follow the visible set: a
                // canvas out of the viewport stops costing server CPU and wire.
                self.on_tree_changed();
            }
            WebEvent::GpuReady { def_id, mut gpu } => {
                // On the web the `<canvas>` is often not laid out yet when
                // `Gpu::new` reads its size (the size is captured before the async
                // adapter/device awaits), so the surface can come up configured to
                // a stale 1x1. Re-read the now-laid-out size and reconfigure before
                // the first frame — otherwise the clear fills the canvas (a gray
                // backdrop) but every widget lays out into a ~0 px area and nothing
                // visible is drawn. A `Resized` that arrived while the GPU was
                // pending was stashed in `pending_size`; prefer the live size and
                // fall back to it.
                let Some(slot) = self.canvases.get_mut(&def_id) else {
                    return; // detached while the device was coming up
                };
                let size = gpu.window.inner_size();
                let (w, h) = if size.width > 0 && size.height > 0 {
                    (size.width, size.height)
                } else {
                    slot.pending_size.unwrap_or((size.width, size.height))
                };
                gpu.resize(w, h);
                let painter = Painter::new(&gpu.device, gpu.config.format);
                let overlay = Painter::new(&gpu.device, gpu.config.format);
                log(&format!(
                    "def {def_id}: GPU device ready; surface {}x{}",
                    gpu.config.width, gpu.config.height
                ));
                slot.render = Some(WindowRender {
                    gpu,
                    painter,
                    overlay,
                    waveforms: HashMap::new(),
                    spectrograms: HashMap::new(),
                });
                let pending = std::mem::take(&mut slot.pending_bulk);
                self.build_resources(def_id);
                // Bulk data that finished downloading while the device was
                // still coming up gets its GPU slots now.
                for (widget_id, data) in pending {
                    self.place_bulk(def_id, widget_id, data);
                }
                self.request_redraw(def_id);
            }
            WebEvent::Inbound(bytes) => self.on_inbound(event_loop, &bytes),
            WebEvent::ConnectServer(url) => match WsServerLink::connect(&url) {
                Ok(link) => {
                    self.host.set_server_link(ServerLink::Ws(link));
                    log(&format!("audio-server leg connecting to {url}"));
                    self.on_server_attached();
                }
                Err(e) => log(&format!("cannot open audio-server WebSocket {url}: {e}")),
            },
            WebEvent::ConnectPage(callback) => {
                self.host
                    .set_server_link(ServerLink::Page(PageServerLink { callback }));
                log("audio-server leg attached to the in-page engine");
                self.on_server_attached();
            }
            WebEvent::ServerInbound(bytes) => self.on_server_inbound(&bytes),
            WebEvent::Tick => self.on_tick(),
            WebEvent::BulkReady {
                def_id,
                widget_id,
                data,
            } => self.on_bulk_ready(def_id, widget_id, data),
            WebEvent::Theme(entries) => {
                for w in self
                    .host
                    .theme
                    .overlay(entries.iter().map(|(k, v)| (k.as_str(), v.as_str())))
                {
                    log(&w);
                }
                // The base changed under the resolved references: re-resolve
                // every window's theme groups over the new host theme.
                let base = std::sync::Arc::new(self.host.theme.clone());
                for id in self.host.window_def_ids() {
                    if let Some(tree) = self.host.window_def_mut(id) {
                        super::widget::resolve_themes(tree, &base);
                    }
                }
                for def in self.canvases.keys().copied().collect::<Vec<_>>() {
                    self.draw(def);
                }
            }
            WebEvent::Metrics(entries) => {
                for w in self
                    .host
                    .metrics
                    .overlay(entries.iter().map(|(k, v)| (k.as_str(), *v)))
                {
                    log(&w);
                }
                // Every canvas re-resolves the new roles at its own scale, and
                // sizes are then read per frame from that one table, so a
                // redraw is the rest of the update.
                self.host.refresh_metrics();
                for def in self.canvases.keys().copied().collect::<Vec<_>>() {
                    self.draw(def);
                }
            }
        }
    }

    /// Every per-canvas event routes by winit's window id: a document's
    /// canvases each get their own pointer, modifiers and repaints.
    fn window_event(&mut self, _event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(def) = self.by_winit.get(&id).copied() else {
            return;
        };
        match event {
            WindowEvent::Resized(size) => {
                let Some(slot) = self.canvases.get_mut(&def) else {
                    return;
                };
                match slot.render.as_mut() {
                    Some(render) => render.gpu.resize(size.width, size.height),
                    // The GPU is still coming up; remember the size so `GpuReady`
                    // can configure the surface to it instead of a stale 1x1.
                    None => slot.pending_size = Some((size.width, size.height)),
                }
                slot.request_redraw();
            }
            WindowEvent::ModifiersChanged(mods) => {
                if let Some(slot) = self.canvases.get_mut(&def) {
                    slot.shift = mods.state().shift_key();
                    slot.ctrl = mods.state().control_key();
                    slot.alt = mods.state().alt_key();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let Some(slot) = self.canvases.get_mut(&def) else {
                    return;
                };
                slot.cursor = (position.x, position.y);
                if slot.gestures.dragging() {
                    self.on_move(def);
                } else if self
                    .host
                    .window_def(def)
                    .is_some_and(Widget::has_hover_readout)
                {
                    // The hover readout follows the pointer (the native rule).
                    self.request_redraw(def);
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => match state {
                ElementState::Pressed => self.on_press(def),
                ElementState::Released => self.on_release(def),
            },
            // A finger drives the same machine a pointer does: the desktop's
            // press → drag → release, with the touch's own position. winit
            // reports touch separately from the pointer events, so without this
            // arm a phone reaches every DOM control on the page and nothing at
            // all inside a canvas.
            WindowEvent::Touch(touch) => {
                let Some(slot) = self.canvases.get_mut(&def) else {
                    return;
                };
                let owned = slot.touch == Some(touch.id);
                match touch.phase {
                    TouchPhase::Started if slot.touch.is_none() => {
                        slot.touch = Some(touch.id);
                        slot.cursor = (touch.location.x, touch.location.y);
                        self.on_press(def);
                    }
                    TouchPhase::Moved if owned => {
                        slot.cursor = (touch.location.x, touch.location.y);
                        if slot.gestures.dragging() {
                            self.on_move(def);
                        }
                    }
                    TouchPhase::Ended | TouchPhase::Cancelled if owned => {
                        slot.touch = None;
                        slot.cursor = (touch.location.x, touch.location.y);
                        self.on_release(def);
                    }
                    // Another finger while one is already down, or a stray
                    // phase for a finger this canvas never claimed.
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64,
                    MouseScrollDelta::PixelDelta(p) => p.y / 50.0,
                };
                self.on_wheel(def, steps);
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let key = event.logical_key.clone();
                self.on_key(def, &key);
            }
            // A canvas out of the viewport is skipped: the browser would not
            // composite it anyway, and *we* would still have computed the frame.
            WindowEvent::RedrawRequested if self.canvases.get(&def).is_some_and(|s| s.visible) => {
                self.draw(def)
            }
            _ => {}
        }
    }
}

thread_local! {
    /// The running app's event-loop proxy, so `resumed` can reach it for the
    /// async GPU hand-off (winit's web loop is single-threaded).
    static WEB_PROXY: RefCell<Option<EventLoopProxy<WebEvent>>> = const { RefCell::new(None) };
}

/// Builds the GPU slot for every inline-data `waveform`/`spectrogram` in the
/// tree (the zero-latency bulk source; `path`/`cache`/`buffer` references load
/// async through [`fetch_bulk`] and the fetch machine).
fn build_inline_timelines(
    widget: &Widget,
    gpu: &Gpu,
    waveforms: &mut HashMap<i32, WaveformSlot>,
    spectrograms: &mut HashMap<i32, SpectrogramSlot>,
) {
    if let Some(id) = widget.id {
        match &widget.kind {
            WidgetKind::Waveform {
                samples,
                base_bucket,
                channels,
                ..
            } if !samples.is_empty() => {
                let data = WaveformData::from_interleaved(samples, *channels, *base_bucket);
                waveforms.insert(id, frame::waveform_slot(data, gpu));
            }
            WidgetKind::Spectrogram {
                samples,
                channels,
                window_size,
                hop,
                sample_rate,
                ..
            } if !samples.is_empty() => {
                let stfts = frame::stft_lanes(
                    frame::deinterleave(samples, *channels),
                    *window_size,
                    *hop,
                    *sample_rate,
                );
                if let Some(slot) = frame::spectrogram_slot(stfts, gpu) {
                    spectrograms.insert(id, slot);
                }
            }
            _ => {}
        }
    }
    for child in &widget.children {
        build_inline_timelines(child, gpu, waveforms, spectrograms);
    }
}

/// Walks the tree collecting the async bulk sources: waveforms referencing a
/// server `buffer` (fetched over the WS leg) and waveform/plot `path`/`cache`
/// references (URLs fetched against the page origin). The browser mirror of
/// the native `collect_waveforms`/`load_plot_paths` resolution, minus the
/// inline case handled by [`build_inline_waveforms`].
fn collect_bulk(
    widget: &Widget,
    buffer_refs: &mut Vec<(i32, i32)>,
    requests: &mut Vec<(i32, BulkRequest)>,
) {
    if let Some(id) = widget.id {
        match &widget.kind {
            WidgetKind::Waveform {
                samples,
                base_bucket,
                buffer,
                path,
                cache,
                channels,
                ..
            } => {
                if let Some(cache) = cache {
                    requests.push((id, BulkRequest::Cache(cache.to_string_lossy().into_owned())));
                } else if let Some(path) = path {
                    requests.push((
                        id,
                        BulkRequest::Raw {
                            url: path.to_string_lossy().into_owned(),
                            channels: *channels,
                            base_bucket: *base_bucket,
                        },
                    ));
                } else if let (Some(bufnum), true) = (buffer, samples.is_empty()) {
                    buffer_refs.push((id, *bufnum));
                }
            }
            WidgetKind::Spectrogram {
                samples,
                channels,
                buffer,
                path,
                cache,
                window_size,
                hop,
                sample_rate,
                ..
            } => {
                if let Some(cache) = cache {
                    requests.push((
                        id,
                        BulkRequest::StftCache(cache.to_string_lossy().into_owned()),
                    ));
                } else if let Some(path) = path {
                    requests.push((
                        id,
                        BulkRequest::StftRaw {
                            url: path.to_string_lossy().into_owned(),
                            channels: *channels,
                            window_size: *window_size,
                            hop: *hop,
                            sample_rate: *sample_rate,
                        },
                    ));
                } else if let (Some(bufnum), true) = (buffer, samples.is_empty()) {
                    buffer_refs.push((id, *bufnum));
                }
            }
            WidgetKind::Plot {
                samples,
                path,
                channels,
                ..
            } => {
                if samples.is_empty()
                    && let Some(path) = path
                {
                    requests.push((
                        id,
                        BulkRequest::Plot {
                            url: path.to_string_lossy().into_owned(),
                            channels: *channels,
                        },
                    ));
                }
            }
            // A clip's take resolves exactly like a waveform's samples (cache →
            // path → buffer), only its landing differs: the tree, not the GPU.
            WidgetKind::Clip {
                samples,
                buffer,
                path,
                cache,
                channels,
                base_bucket,
                ..
            } => {
                if let Some(cache) = cache {
                    requests.push((id, BulkRequest::Cache(cache.to_string_lossy().into_owned())));
                } else if let Some(path) = path {
                    requests.push((
                        id,
                        BulkRequest::Raw {
                            url: path.to_string_lossy().into_owned(),
                            channels: *channels,
                            base_bucket: *base_bucket,
                        },
                    ));
                } else if let (Some(bufnum), true) = (buffer, samples.is_empty()) {
                    buffer_refs.push((id, *bufnum));
                }
            }
            _ => {}
        }
    }
    for child in &widget.children {
        collect_bulk(child, buffer_refs, requests);
    }
}

/// Fetches one bulk URL and decodes it off the event loop, then hands the
/// result back through the proxy as [`WebEvent::BulkReady`].
async fn fetch_bulk(def_id: i32, widget_id: i32, request: BulkRequest) {
    let url = match &request {
        BulkRequest::Cache(url) | BulkRequest::StftCache(url) => url,
        BulkRequest::Raw { url, .. }
        | BulkRequest::StftRaw { url, .. }
        | BulkRequest::Plot { url, .. } => url,
    }
    .clone();
    let bytes = match fetch_bytes(&url).await {
        Ok(bytes) => bytes,
        Err(e) => return log(&format!("bulk fetch {url}: {e}")),
    };
    let data = match request {
        BulkRequest::Cache(_) => {
            let Some(multi) = MultiPyramid::from_bytes(&bytes) else {
                return log(&format!("bulk fetch {url}: malformed peak pyramid"));
            };
            log(&format!(
                "waveform: fetched peak cache {url} ({} samples x {} channel(s), no raw data)",
                multi.frames(),
                multi.num_channels()
            ));
            BulkData::Waveform(WaveformData::with_multi_pyramid(multi))
        }
        BulkRequest::Raw {
            channels,
            base_bucket,
            ..
        } => {
            let flat = decode_f32(&bytes);
            log(&format!(
                "waveform: fetched {} samples x {channels} channel(s) from {url} (pyramids built in wasm)",
                flat.len() / channels.max(1)
            ));
            BulkData::Waveform(WaveformData::from_interleaved(&flat, channels, base_bucket))
        }
        BulkRequest::StftCache(_) => {
            let Some(stft) = Stft::from_bytes(&bytes) else {
                return log(&format!("bulk fetch {url}: malformed STFT cache"));
            };
            log(&format!(
                "spectrogram: fetched STFT cache {url} ({} frames x {} bins)",
                stft.n_frames(),
                stft.n_bins()
            ));
            BulkData::Spectrogram(vec![stft])
        }
        BulkRequest::StftRaw {
            channels,
            window_size,
            hop,
            sample_rate,
            ..
        } => {
            let flat = decode_f32(&bytes);
            let stfts = frame::stft_lanes(
                frame::deinterleave(&flat, channels),
                window_size,
                hop,
                sample_rate,
            );
            log(&format!(
                "spectrogram: fetched {} samples x {channels} channel(s) from {url} (STFT in wasm)",
                flat.len() / channels.max(1)
            ));
            BulkData::Spectrogram(stfts)
        }
        BulkRequest::Plot { channels, .. } => {
            let mut flat = decode_f32(&bytes);
            let channels = channels.max(1);
            flat.truncate(flat.len() / channels * channels);
            log(&format!(
                "plot: fetched {} samples x {channels} channel(s) from {url}",
                flat.len() / channels
            ));
            BulkData::Plot(flat.into())
        }
    };
    if let Some(proxy) = WEB_PROXY.with(|p| p.borrow().clone()) {
        let _ = proxy.send_event(WebEvent::BulkReady {
            def_id,
            widget_id,
            data,
        });
    }
}

/// One `fetch` of `url` to raw bytes (an `ArrayBuffer`), erroring on a non-2xx
/// status so a missing resource is visible instead of decoding garbage.
async fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    use wasm_bindgen_futures::JsFuture;
    let window = web_sys::window().ok_or("no window")?;
    let response = JsFuture::from(window.fetch_with_str(url))
        .await
        .map_err(|e| format!("{e:?}"))?;
    let response: web_sys::Response = response.dyn_into().map_err(|_| "not a Response")?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let buffer = JsFuture::from(response.array_buffer().map_err(|e| format!("{e:?}"))?)
        .await
        .map_err(|e| format!("{e:?}"))?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}

/// Decodes raw little-endian `f32` bytes flat (interleaved as sent) — the
/// multichannel views de-interleave downstream.
fn decode_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
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

    /// Gives one `window`-rooted def its own `<canvas>`, which the caller
    /// created and the document places.
    ///
    /// This is the browser's answer to the desktop's window manager: on the
    /// desktop `clausters-gui` opens a window per def and the system places it;
    /// in a tab the canvas is an element and **the document places it** — CSS,
    /// the order of the markup. Attach before feeding the def's `/gui_def`, so
    /// the first frame draws into the right surface. Attaching a def that
    /// already has a canvas replaces it.
    ///
    /// A page that never calls this still works: a `/gui_def` with no canvas
    /// gets one appended to `<body>`, the older single-canvas posture.
    pub fn attach(&self, def_id: i32, canvas: web_sys::HtmlCanvasElement) {
        let _ = self.proxy.send_event(WebEvent::Attach {
            def_id,
            canvas: Some(canvas),
        });
    }

    /// Frees a def's canvas: its GPU surface and every derived resource go. The
    /// `<canvas>` element itself is the page's, to remove or reuse.
    pub fn detach(&self, def_id: i32) {
        let _ = self.proxy.send_event(WebEvent::Detach(def_id));
    }

    /// Sizes a canvas in **device pixels**, with the **scale** those pixels were
    /// measured at — a component's `ResizeObserver` box times
    /// `devicePixelRatio`, and that ratio. The host never reads the DOM: the
    /// element owns its box and reports the pixels.
    ///
    /// Both halves are needed and neither substitutes for the other. The
    /// backing store is device pixels, so the surface takes the product; the
    /// widget sizes a GuiDef declares are **logical**, so resolving them takes
    /// the ratio — and a product cannot be un-multiplied. A page that already
    /// scales its box by `devicePixelRatio` passes the same ratio here.
    pub fn resize(&self, def_id: i32, width: u32, height: u32, scale: f32) {
        let _ = self.proxy.send_event(WebEvent::Resize {
            def_id,
            width,
            height,
            scale,
        });
    }

    /// Tells the host whether a canvas is in the viewport (a component's
    /// `IntersectionObserver`).
    ///
    /// A hidden canvas is skipped on the tick and its buses leave the
    /// `/bus_stream`/`/bus_tapStream` sets — a document can hold fifty canvases with
    /// three in view, and neither this host nor the server should be working
    /// for the other forty-seven.
    pub fn set_visible(&self, def_id: i32, visible: bool) {
        let _ = self
            .proxy
            .send_event(WebEvent::SetVisible { def_id, visible });
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

    /// Attaches the host's audio-server leg to the **in-page engine**: every
    /// outbound OSC packet (bound-widget values, `/bus_stream`/`/bus_tapStream`
    /// subscriptions, buffer fetches, `/clock_query`) is handed to `send` as a
    /// `Uint8Array`; the page forwards it to the engine and feeds the engine's
    /// replies back through [`server_reply`](Self::server_reply).
    pub fn connect_page(&self, send: js_sys::Function) {
        let _ = self.proxy.send_event(WebEvent::ConnectPage(send));
    }

    /// Overlays the host's color theme from a JSON object of
    /// `{"role": "#rrggbb[aa]"}` entries — the browser form of the native
    /// `[gui.theme]` config table. A partial object is fine; unknown roles or
    /// bad colors are logged and skipped.
    pub fn theme(&self, json: &str) {
        match serde_json::from_str::<std::collections::BTreeMap<String, String>>(json) {
            Ok(table) => {
                let _ = self
                    .proxy
                    .send_event(WebEvent::Theme(table.into_iter().collect()));
            }
            Err(e) => log(&format!("cannot parse theme JSON: {e}")),
        }
    }

    /// Overlays the host's size metrics from a JSON object of
    /// `{"role": number}` entries — the browser form of the native
    /// `[gui.metrics]` config table, the reserved `scale` density key included.
    /// A partial object is fine; unknown roles or unusable numbers are logged
    /// and skipped.
    pub fn metrics(&self, json: &str) {
        match serde_json::from_str::<std::collections::BTreeMap<String, f64>>(json) {
            Ok(table) => {
                let _ = self
                    .proxy
                    .send_event(WebEvent::Metrics(table.into_iter().collect()));
            }
            Err(e) => log(&format!("cannot parse metrics JSON: {e}")),
        }
    }

    /// Feeds one reply packet from the in-page engine (a streamed `/bus_set`, a
    /// `/bus_tapStream.reply`, a `/buffer_query.reply`/`/buffer_getRange.reply`, a `/clock_query.reply`) into the host —
    /// the inbound half of [`connect_page`](Self::connect_page), the same
    /// dispatch the WS leg's `onmessage` uses.
    pub fn server_reply(&self, packet: &[u8]) {
        let _ = self
            .proxy
            .send_event(WebEvent::ServerInbound(packet.to_vec()));
    }
}

/// The ordered boot packets of a persisted bundle, for the page to send to the
/// in-page engine: `synthdefs`/`graphdefs` are arrays of `Uint8Array` (each
/// file's bytes verbatim), `boot_json` the optional `boot.json` text,
/// `guidef_tree` the GuiDef tree JSON (its root `boot` messages run last).
/// Returns an array of `Uint8Array` packets ending in `/server_sync sync_id+1` — the
/// page knows the bundle is up when `/server_sync.reply sync_id+1` comes back. The
/// ordering/encoding logic lives in the platform-agnostic `host::bundle`
/// module, natively unit-tested.
#[wasm_bindgen]
pub fn bundle_boot_packets(
    synthdefs: js_sys::Array,
    graphdefs: js_sys::Array,
    boot_json: Option<String>,
    guidef_tree: &str,
    sync_id: i32,
) -> js_sys::Array {
    let to_bytes = |array: js_sys::Array| -> Vec<Vec<u8>> {
        array
            .iter()
            .map(|v| js_sys::Uint8Array::new(&v).to_vec())
            .collect()
    };
    let packets = super::bundle::boot_packets(
        &to_bytes(synthdefs),
        &to_bytes(graphdefs),
        boot_json.as_ref().map(|s| s.as_bytes()),
        guidef_tree.as_bytes(),
        sync_id,
    );
    packets
        .into_iter()
        .map(|bytes| js_sys::Uint8Array::from(bytes.as_slice()))
        .collect()
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
