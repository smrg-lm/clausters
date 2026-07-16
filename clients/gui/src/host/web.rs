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
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
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
use super::gestures::{GestureCtx, GestureEffect, Gestures};
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
/// requests (`/c_stream`, `/b_query`, `/b_getn`); inbound frames (the server's
/// replies and streamed `/c_set` snapshots) are forwarded into the event loop
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
        // (a streamed `/c_set`, a `/b_info`/`/b_setn` reply, a `/fail`),
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
    /// One inbound OSC packet from the audio server over the WS leg (a streamed
    /// `/c_set`, a `/b_info`/`/b_setn` reply, a `/fail`).
    ServerInbound(Vec<u8>),
    /// The animation tick (a `setInterval` at ~30 fps while the window has live
    /// widgets): advance the scope histories and repaint.
    Tick,
    /// A `fetch` of a waveform/plot URL completed and decoded (the browser's
    /// bulk path: `path`/`cache` resolve against the page origin).
    BulkReady { widget_id: i32, data: BulkData },
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

/// The per-window GPU resources (the browser has a single window/canvas).
struct WindowRender {
    gpu: Gpu,
    painter: Painter,
    /// The editor-chrome overlay pass (selection, playhead, rulers, readout).
    overlay: Painter,
    waveforms: HashMap<i32, WaveformSlot>,
    spectrograms: HashMap<i32, SpectrogramSlot>,
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
    /// A canvas size from a `Resized` that arrived before the GPU was ready (so
    /// `render` was `None` and it could not be applied yet); replayed on
    /// `GpuReady` so the surface is configured to the real size for the first
    /// frame, not a stale 1x1.
    pending_size: Option<(u32, u32)>,
    cursor: (f64, f64),
    /// This canvas' gesture state — the shared machine both fronts drive.
    gestures: Gestures,
    /// Modifier keys (winit `ModifiersChanged`), snapshotted into each
    /// [`GestureCtx`] so Shift-pan/Ctrl-edit/Alt-select work as on the desktop.
    shift: bool,
    ctrl: bool,
    alt: bool,
    /// The piano-roll note clipboard (Ctrl+C/X/V), page-wide.
    clipboard: Vec<pianoroll::Note>,
    /// Live control-bus values streamed from the audio server (`/c_stream` →
    /// `/c_set`), the browser's [`BusSource`] for meters/scopes/canvases.
    buses: Arc<StreamedBuses>,
    /// Recent control-bus samples per `scope` widget id (oldest .. newest),
    /// advanced on [`WebEvent::Tick`] exactly as the native tick does.
    scopes: HashMap<i32, VecDeque<f32>>,
    /// The bus set currently subscribed with `/c_stream` (sorted), so a tree
    /// change only resubscribes when the set actually changed.
    streamed: Vec<i32>,
    /// The newest `/tap_data` window per tap — the browser's source for
    /// audio-rate scopes, read on the tick exactly as the native front reads
    /// the segment's tap rings.
    taps: Arc<StreamedTaps>,
    /// Triggered display window per audio-rate scope widget id, refreshed on
    /// the tick (`live::update_tap_windows`). Also holds each phasescope's
    /// interleaved L/R window (ids do not collide).
    tap_windows: HashMap<i32, Vec<f32>>,
    /// Persistent FFT analysis state per `spectrum` widget id, advanced on the
    /// tick (`live::update_spectra`), exactly as the native front does.
    spectra: HashMap<i32, SpectrumState>,
    /// The `(taps, window frames)` currently subscribed with `/tap_stream`,
    /// so a tree change only resubscribes when they actually changed.
    tap_streamed: (Vec<i32>, usize),
    /// The server's sample rate (from `/clock.reply`, requested when the leg
    /// connects); `0.0` until known — window sizing then assumes 48 kHz.
    server_rate: f64,
    /// The engine's sample clock from the newest `/clock.reply` — the browser
    /// playhead source (polled once per tick while a playhead is shown; the
    /// native front reads the shm header instead).
    server_clock: f64,
    /// The animation tick: the `setInterval` id and its closure, kept alive
    /// while the current def has live widgets (meter/scope/canvas).
    tick: Option<(i32, Closure<dyn FnMut()>)>,
    /// Whether the first streamed `/c_set` snapshot was logged (one line as
    /// evidence the bus stream is flowing; logging every frame would spam).
    stream_seen: bool,
    /// The server-buffer fetch machine (`/b_query` → chunked `/b_getn`),
    /// shared with the native front; requests ride the WS leg.
    fetches: BufferFetches,
    /// Fetched waveforms/spectrograms that arrived before the GPU was ready,
    /// placed on `GpuReady` (plots need no GPU and are placed immediately).
    pending_bulk: Vec<(i32, BulkData)>,
}

impl WebApp {
    fn new(outbox: Rc<RefCell<VecDeque<Vec<u8>>>>) -> Self {
        Self {
            host: Host::new(),
            outbox,
            window: None,
            render: None,
            current_def: None,
            pending_size: None,
            cursor: (0.0, 0.0),
            gestures: Gestures::default(),
            shift: false,
            ctrl: false,
            alt: false,
            clipboard: Vec::new(),
            buses: Arc::new(StreamedBuses::default()),
            scopes: HashMap::new(),
            streamed: Vec::new(),
            taps: Arc::new(StreamedTaps::default()),
            tap_windows: HashMap::new(),
            spectra: HashMap::new(),
            tap_streamed: (Vec::new(), 0),
            server_rate: 0.0,
            server_clock: 0.0,
            tick: None,
            stream_seen: false,
            fetches: BufferFetches::default(),
            pending_bulk: Vec::new(),
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
                    self.scopes.clear();
                    self.tap_windows.clear();
                    self.spectra.clear();
                    self.pending_bulk.clear();
                    self.fetches.drop_def(id); // rebuild semantics on a re-/gui_def
                    if self.render.is_some() {
                        self.build_resources();
                        self.request_redraw();
                    }
                    self.start_bulk(id);
                    self.on_tree_changed();
                }
                HostEffect::CloseWindow(id) => {
                    if self.current_def == Some(id) {
                        self.current_def = None;
                        self.scopes.clear();
                        self.tap_windows.clear();
                        self.spectra.clear();
                        self.pending_bulk.clear();
                        self.fetches.drop_def(id);
                        if let Some(r) = self.render.as_mut() {
                            r.waveforms.clear();
                            r.spectrograms.clear();
                        }
                        self.request_redraw();
                        self.on_tree_changed();
                    }
                }
                HostEffect::Redraw(id) => {
                    if self.current_def == Some(id) {
                        // A `/gui_set` may have retargeted a meter/scope `bus`:
                        // re-derive the subscription (a no-op when unchanged).
                        self.on_tree_changed();
                        self.request_redraw();
                    }
                }
            }
        }
    }

    /// Re-derives everything that follows from the current tree's live widgets:
    /// the `/c_stream` and `/tap_stream` subscriptions on the WS leg and the
    /// animation tick. Called on open/close/redraw and after the server leg
    /// attaches; cheap (a tree walk) and idempotent, so calling it eagerly is
    /// fine.
    fn on_tree_changed(&mut self) {
        self.sync_bus_stream();
        self.sync_tap_stream();
        self.ensure_tick();
    }

    /// Subscribes the audio server to exactly the control buses the current
    /// tree reads live (`/c_stream`, replacing this client's previous
    /// subscription), or cancels when none are left. Skipped without a server
    /// leg; `ConnectServer` re-runs it once the leg exists.
    fn sync_bus_stream(&mut self) {
        let mut wanted = Vec::new();
        if let Some(def) = self.current_def
            && let Some(tree) = self.host.window_def(def)
        {
            live::collect_live_buses(tree, &mut wanted);
        }
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
            addr: "/c_stream".into(),
            args,
        }) {
            Ok(()) => {
                log(&format!("/c_stream subscription: {wanted:?}"));
                self.streamed = wanted;
            }
            Err(e) => log(&format!("failed to (re)subscribe /c_stream: {e}")),
        }
    }

    /// Subscribes the audio server to exactly the audio taps the current
    /// tree's oscilloscopes read (`/tap_stream`, replacing this client's
    /// previous subscription), sized to the largest raw window any of them
    /// needs; cancels when none are left. Skipped without a server leg.
    fn sync_tap_stream(&mut self) {
        let mut wanted = Vec::new();
        let mut frames = 0usize;
        if let Some(def) = self.current_def
            && let Some(tree) = self.host.window_def(def)
        {
            live::collect_live_taps(tree, &mut wanted);
            // Size the stream window to the largest any tap consumer needs — the
            // oscilloscopes, the phasescopes and the spectra alike.
            frames = live::tap_stream_frames(tree, self.server_rate);
        }
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
            addr: "/tap_stream".into(),
            args,
        }) {
            Ok(()) => {
                log(&format!("/tap_stream subscription: {wanted:?} x{frames}"));
                self.tap_streamed = (wanted, frames);
            }
            Err(e) => log(&format!("failed to (re)subscribe /tap_stream: {e}")),
        }
    }

    /// Starts or stops the ~30 fps animation tick to match the current tree:
    /// running while it has live widgets (meter/scope/canvas), stopped
    /// otherwise. The tick advances the scope histories and repaints — the
    /// browser twin of the native `about_to_wait` frame timer, driven by
    /// `setInterval` because `std::time::Instant` does not exist on wasm.
    fn ensure_tick(&mut self) {
        let animated = self
            .current_def
            .and_then(|def| self.host.window_def(def))
            .is_some_and(|tree| live::tree_has_canvas(tree) || live::tree_has_live_widget(tree));
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
    /// from the `/tap_data` store (time-based, exactly like the native tick),
    /// then repaint.
    fn on_tick(&mut self) {
        if let Some(def) = self.current_def
            && let Some(tree) = self.host.window_def(def)
        {
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
                &mut self.scopes,
            );
            let taps = &self.taps;
            live::update_tap_windows(
                tree,
                self.server_rate,
                |tap, out| taps.read_raw(tap, out),
                &mut self.tap_windows,
            );
            live::update_phase_windows(
                tree,
                self.server_rate,
                |tap, out| taps.read_raw(tap, out),
                &mut self.tap_windows,
            );
            live::update_spectra(tree, |tap, out| taps.read_raw(tap, out), &mut self.spectra);
            // A visible playhead needs the engine clock: poll it once per tick
            // (the browser's stand-in for the shm header's sample clock).
            if live::tree_has_playhead(tree)
                && let Some(server) = self.host.server()
            {
                let _ = server.send(OscMessage {
                    addr: "/clock".into(),
                    args: vec![],
                });
            }
        }
        self.request_redraw();
    }

    /// Routes one decoded OSC packet from the audio server (the WS leg): the
    /// streamed `/c_set` snapshots into [`StreamedBuses`], the buffer replies
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
            "/c_set" => {
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
            "/b_info" => {
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
            "/b_setn" => {
                let step = self.fetches.on_data(&msg.args);
                self.apply_fetch_step(step);
            }
            "/tap_data" => {
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
            "/clock.reply" => {
                // (samples, rate, ...): keep the rate for window sizing and
                // the sample counter for the timeline playhead.
                if let Some(OscType::Long(samples)) = msg.args.first() {
                    self.server_clock = *samples as f64;
                }
                if let Some(OscType::Double(rate)) = msg.args.get(1) {
                    self.server_rate = *rate;
                    // Window sizes may change with the real rate known.
                    self.sync_tap_stream();
                }
            }
            "/fail" => log(&format!("audio server replied /fail: {:?}", msg.args)),
            // `/done` acks (e.g. for `/c_stream`) need no action.
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
                    if self.current_def != Some(want.def_id) {
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
                            self.place_bulk(want.widget_id, BulkData::Waveform(data));
                        }
                        WidgetKind::Clip { base_bucket, .. } => {
                            // A clip's take lands in the tree (its lane body is
                            // flat geometry decimated from the pyramid, no GPU
                            // slot) — the same landing the mapped bulk path uses.
                            let data =
                                WaveformData::from_interleaved(&samples, channels, base_bucket);
                            self.set_clip_body(want.def_id, want.widget_id, data);
                            continue;
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
                            self.place_bulk(want.widget_id, BulkData::Spectrogram(stfts));
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
                }
                self.request_redraw();
            }
            FetchStep::None => {}
        }
    }

    /// Sends one fetch-machine message over the WS leg (`/b_query`, `/b_getn`),
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
            wasm_bindgen_futures::spawn_local(fetch_bulk(widget_id, request));
        }
    }

    /// Places a decoded GPU-bound resource (waveform or spectrogram): a slot
    /// right away when the device is up, else stashed and replayed on
    /// `GpuReady`.
    fn place_bulk(&mut self, widget_id: i32, data: BulkData) {
        let Some(render) = self.render.as_mut() else {
            self.pending_bulk.push((widget_id, data));
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
    fn on_bulk_ready(&mut self, widget_id: i32, data: BulkData) {
        // A waveform resource wanted by a `clip` lands in the tree, not the GPU.
        if let BulkData::Waveform(_) = &data
            && let Some(def) = self.current_def
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
            self.request_redraw();
            return;
        }
        match data {
            BulkData::Waveform(_) | BulkData::Spectrogram(_) => {
                self.place_bulk(widget_id, data);
            }
            BulkData::Plot(samples) => {
                if let Some(def) = self.current_def
                    && let Some(root) = self.host.window_def_mut(def)
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
        self.request_redraw();
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

    /// (Re)builds the GPU resources for the current def: the inline-data
    /// waveform/spectrogram views (`path`/`cache`/`buffer` references load
    /// async through [`fetch_bulk`] and the fetch machine).
    fn build_resources(&mut self) {
        let Some(def) = self.current_def else { return };
        let Some(render) = self.render.as_ref() else {
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
        if let Some(render) = self.render.as_mut() {
            render.waveforms = waveforms;
            render.spectrograms = spectrograms;
        }
        for (id, total) in totals {
            self.host.set_timeline_total(id, total);
        }
    }

    /// Renders the current def through the shared frame path. The live inputs
    /// come from the streamed buses (meters/canvases read them in `render`, the
    /// scopes their tick-fed histories); the node tree stays empty until a
    /// browser node-tree path exists.
    fn draw(&mut self) {
        let Some(def) = self.current_def else { return };
        let active_button = self.gestures.active_button();
        let server_attached = self.host.server().is_some();
        let Some(tree) = self.host.window_def(def) else {
            return;
        };
        let inputs = frame::FrameInputs {
            bus: Some(self.buses.as_ref() as &dyn BusSource),
            active_button,
            server_attached,
            sample_rate: self.server_rate,
            sample_clock: self.server_clock,
            cursor: Some(self.cursor),
            timelines: self.host.timelines(),
            // A rewiring drag in flight draws its wire to the pointer.
            wiring: self
                .gestures
                .wiring()
                .map(|(id, port)| (id, port, (self.cursor.0 as f32, self.cursor.1 as f32))),
            ..Default::default()
        };
        let Some(render) = self.render.as_mut() else {
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
            &self.scopes,
            &self.tap_windows,
            &self.spectra,
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

    /// Snapshots the gesture context for the current def: framebuffer size,
    /// modifier keys, and the heavy views' lane counts (channel/lane splits
    /// live in this front's GPU slots, so they are copied out here) — the
    /// browser twin of the native front's snapshot.
    fn gesture_ctx(&self, def: i32) -> GestureCtx {
        let (fb_w, fb_h) = self.fb();
        let mut ctx = GestureCtx::new(def, fb_w, fb_h);
        ctx.shift = self.shift;
        ctx.ctrl = self.ctrl;
        ctx.alt = self.alt;
        if let Some(render) = self.render.as_ref() {
            for (id, slot) in &render.waveforms {
                ctx.wave_lanes.insert(*id, slot.view.num_channels());
            }
            for (id, slot) in &render.spectrograms {
                ctx.spect_lanes.insert(*id, slot.views.len());
            }
        }
        ctx
    }

    /// Carries out a gesture's effects over this front's sinks: `/gui_event`s
    /// to the page outbox (a bound widget already forwarded inside the
    /// machine), repaints of the one canvas. There is no pointer grab in the
    /// browser (the grab callback returns `false`), so releases are no-ops.
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
                // One canvas: every repaint lands on it, whichever window a
                // linked-view mutation names.
                GestureEffect::Redraw(_) => self.request_redraw(),
                GestureEffect::ReleasePointer(_) => {}
            }
        }
    }

    /// Pointer press: the shared gesture machine acts by widget kind.
    fn on_press(&mut self) {
        let Some(def) = self.current_def else { return };
        let (cx, cy) = self.cursor;
        let ctx = self.gesture_ctx(def);
        let effects = self
            .gestures
            .press(&mut self.host, &ctx, cx, cy, &mut || false);
        self.apply_gesture_effects(effects);
    }

    /// Pointer move while dragging: the machine drives the dragged target.
    fn on_move(&mut self) {
        let Some(def) = self.current_def else { return };
        let (cx, cy) = self.cursor;
        let ctx = self.gesture_ctx(def);
        let effects = self.gestures.drag_to(&mut self.host, &ctx, cx, cy);
        self.apply_gesture_effects(effects);
    }

    /// Pointer release: the machine finishes the drag (button up, wire landing).
    fn on_release(&mut self) {
        let Some(def) = self.current_def else { return };
        let (cx, cy) = self.cursor;
        let ctx = self.gesture_ctx(def);
        let effects = self.gestures.release(&mut self.host, &ctx, cx, cy);
        self.apply_gesture_effects(effects);
    }

    /// Wheel: the machine zooms the time axis or the vertical display window.
    fn on_wheel(&mut self, steps: f64) {
        let Some(def) = self.current_def else { return };
        let (cx, cy) = self.cursor;
        let ctx = self.gesture_ctx(def);
        let effects = self.gestures.wheel(&mut self.host, &ctx, cx, cy, steps);
        self.apply_gesture_effects(effects);
    }

    /// Keyboard: the same editing operations the desktop front maps (Delete,
    /// Ctrl+C/X/V, `q` quantize, `r` reset) — minus Escape, which closes an OS
    /// window there but has no window to close here.
    fn on_key(&mut self, key: &Key) {
        let Some(def) = self.current_def else { return };
        let (cx, cy) = self.cursor;
        let ctx = self.gesture_ctx(def);
        let effects = match key {
            Key::Named(NamedKey::Delete) | Key::Named(NamedKey::Backspace) => {
                self.gestures.delete_selected(&mut self.host, &ctx, cx, cy)
            }
            Key::Character(c) if self.ctrl && c.eq_ignore_ascii_case("c") => self
                .gestures
                .copy_selected(&mut self.host, &ctx, cx, cy, false, &mut self.clipboard),
            Key::Character(c) if self.ctrl && c.eq_ignore_ascii_case("x") => self
                .gestures
                .copy_selected(&mut self.host, &ctx, cx, cy, true, &mut self.clipboard),
            Key::Character(c) if self.ctrl && c.eq_ignore_ascii_case("v") => self
                .gestures
                .paste_at_cursor(&mut self.host, &ctx, cx, cy, &self.clipboard),
            Key::Character(c) if c.eq_ignore_ascii_case("q") => {
                self.gestures.quantize(&mut self.host, &ctx, cx, cy)
            }
            Key::Character(c) if c.eq_ignore_ascii_case("r") => {
                self.gestures.reset_timelines(&mut self.host, &ctx)
            }
            _ => return,
        };
        self.apply_gesture_effects(effects);
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
        log("opened window over <canvas>; requesting GPU adapter (WebGPU, else WebGL2)...");
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
                    // No GPU adapter at all (neither WebGPU nor WebGL2): surface a
                    // clear, actionable message instead of aborting; the canvas
                    // stays blank but the page survives.
                    log(&e);
                    set_status(&e);
                }
            }
        });
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: WebEvent) {
        match event {
            WebEvent::GpuReady(mut gpu) => {
                // On the web the `<canvas>` is often not laid out yet when
                // `Gpu::new` reads its size (the size is captured before the async
                // adapter/device awaits), so the surface can come up configured to
                // a stale 1x1. Re-read the now-laid-out size and reconfigure before
                // the first frame — otherwise the clear fills the canvas (a gray
                // backdrop) but every widget lays out into a ~0 px area and nothing
                // visible is drawn. A `Resized` that arrived while the GPU was
                // pending was stashed in `pending_size`; prefer the live size and
                // fall back to it.
                let size = gpu.window.inner_size();
                let (w, h) = if size.width > 0 && size.height > 0 {
                    (size.width, size.height)
                } else {
                    self.pending_size.unwrap_or((size.width, size.height))
                };
                gpu.resize(w, h);
                let painter = Painter::new(&gpu.device, gpu.config.format);
                let overlay = Painter::new(&gpu.device, gpu.config.format);
                log(&format!(
                    "GPU device ready; surface {}x{}",
                    gpu.config.width, gpu.config.height
                ));
                self.render = Some(WindowRender {
                    gpu,
                    painter,
                    overlay,
                    waveforms: HashMap::new(),
                    spectrograms: HashMap::new(),
                });
                if self.current_def.is_some() {
                    self.build_resources();
                }
                // Bulk data that finished downloading while the device was
                // still coming up gets its GPU slots now.
                for (widget_id, data) in std::mem::take(&mut self.pending_bulk) {
                    self.place_bulk(widget_id, data);
                }
                self.request_redraw();
            }
            WebEvent::Inbound(bytes) => self.on_inbound(&bytes),
            WebEvent::ConnectServer(url) => match WsServerLink::connect(&url) {
                Ok(link) => {
                    self.host.set_server_link(ServerLink::Ws(link));
                    log(&format!("audio-server leg connecting to {url}"));
                    // A fresh connection holds no subscription: forget the old
                    // ones and subscribe the current tree's buses and taps
                    // (frames queue until the socket opens, so sending now is
                    // safe). `/clock` fetches the rate the oscilloscope
                    // windows are sized with.
                    self.streamed.clear();
                    self.tap_streamed = (Vec::new(), 0);
                    if let Some(server) = self.host.server() {
                        let _ = server.send(OscMessage {
                            addr: "/clock".into(),
                            args: vec![],
                        });
                    }
                    self.on_tree_changed();
                }
                Err(e) => log(&format!("cannot open audio-server WebSocket {url}: {e}")),
            },
            WebEvent::ServerInbound(bytes) => self.on_server_inbound(&bytes),
            WebEvent::Tick => self.on_tick(),
            WebEvent::BulkReady { widget_id, data } => self.on_bulk_ready(widget_id, data),
        }
    }

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::Resized(size) => {
                if let Some(render) = self.render.as_mut() {
                    render.gpu.resize(size.width, size.height);
                } else {
                    // The GPU is still coming up; remember the size so `GpuReady`
                    // can configure the surface to it instead of a stale 1x1.
                    self.pending_size = Some((size.width, size.height));
                }
                self.request_redraw();
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.shift = mods.state().shift_key();
                self.ctrl = mods.state().control_key();
                self.alt = mods.state().alt_key();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                if self.gestures.dragging() {
                    self.on_move();
                } else if self
                    .current_def
                    .and_then(|def| self.host.window_def(def))
                    .is_some_and(Widget::has_hover_readout)
                {
                    // The hover readout follows the pointer (the native rule).
                    self.request_redraw();
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
            WindowEvent::MouseWheel { delta, .. } => {
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64,
                    MouseScrollDelta::PixelDelta(p) => p.y / 50.0,
                };
                self.on_wheel(steps);
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let key = event.logical_key.clone();
                self.on_key(&key);
            }
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
async fn fetch_bulk(widget_id: i32, request: BulkRequest) {
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
        let _ = proxy.send_event(WebEvent::BulkReady { widget_id, data });
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
