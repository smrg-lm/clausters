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

use std::collections::{HashMap, VecDeque};
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

use clausters_core::osc::{OscMessage, OscPacket, OscType, encode};
use tracing::{debug, info, warn};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{
    DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, DeviceEvents, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::gpu::Gpu;
use crate::view::TimelineView;
use crate::viewport::View;
use crate::waveform::WaveformData;

use super::bulk::MmapLoader;
use super::canvas::CanvasView;
use super::fetch::{BufferFetches, FetchStep, WaveWant};
use super::frame::{self, WaveformSlot};
use super::interact::{self, slider_t, value_of};
use super::layout::Rect;
use super::live::{collect_scopes, push_sample, tree_has_canvas, tree_has_live_widget};
use super::nodetree::NodeTree;
use super::paint::Painter;
use super::spectrum::SpectrumState;
use super::widget::{Widget, WidgetKind};
use super::{BulkLoader, BusSource, ClientId, GUI_CLOSED, GUI_EVENT, Host, HostEffect, controls};

/// Repaint period for windows with live (shared-memory-backed) widgets — ~30 fps,
/// enough for smooth meters/scopes without spinning the CPU.
const FRAME: Duration = Duration::from_millis(33);
/// How often a window with a `nodetree` re-queries the server's tree. Node
/// creation/removal is caught immediately through `/n_go`/`/n_end`; this low-rate
/// poll picks up `/n_set` control changes (which raise no notification).
const NODETREE_POLL: Duration = Duration::from_millis(200);

/// What the background transport threads hand the main (winit) thread.
#[derive(Debug)]
pub enum UserEvent {
    /// One OSC datagram from a script and where it came from (decoded on the main
    /// thread, through the single shared door, to keep all logic on one thread).
    Osc { from: SocketAddr, bytes: Vec<u8> },
    /// One OSC reply from the audio server (the client leg): `/b_info`, `/b_setn`.
    ServerOsc { bytes: Vec<u8> },
}

/// Runs the windowed host: spawn the transport thread(s), map the shared segment
/// if one was given, then own the winit event loop on this (main) thread until
/// the process is stopped. `shm_path` is the audio server's `--shm` segment, read
/// each frame for meters/scopes; `None` leaves those views reading zero.
pub fn run(host: Host, socket: Arc<UdpSocket>, shm_path: Option<String>) -> Result<(), String> {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|e| format!("cannot create the window event loop ({e}); use --headless on a machine with no display"))?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();
    // The script -> host front.
    let recv_socket = Arc::clone(&socket);
    let script_proxy = proxy.clone();
    std::thread::Builder::new()
        .name("clausters-gui-osc".into())
        .spawn(move || transport_loop(recv_socket, script_proxy))
        .map_err(|e| e.to_string())?;
    // The host <- audio-server reply path: a background thread only for the UDP
    // leg (the embed link is polled in the event loop, no socket to drain).
    if let Some(leg_socket) = host.server().and_then(|s| s.udp_socket()) {
        std::thread::Builder::new()
            .name("clausters-gui-server".into())
            .spawn(move || server_reply_loop(leg_socket, proxy))
            .map_err(|e| e.to_string())?;
    }

    let shm = open_shm(shm_path);
    let mut app = App::new(host, socket, shm);
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

/// Drains the client leg's socket, forwarding the audio server's replies to the
/// main thread (which routes `/b_info`/`/b_setn` into the buffer-fetch path).
fn server_reply_loop(socket: Arc<UdpSocket>, proxy: EventLoopProxy<UserEvent>) {
    let mut buf = vec![0u8; 65536];
    loop {
        match socket.recv_from(&mut buf) {
            Ok((0, _)) => {}
            Ok((len, _)) => {
                let event = UserEvent::ServerOsc {
                    bytes: buf[..len].to_vec(),
                };
                if proxy.send_event(event).is_err() {
                    return;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {}
            Err(_) => return,
        }
    }
}

/// Maps the audio server's shared segment read-only (Unix only), for the
/// zero-message meters/scopes. A failure is logged and treated as "no segment".
#[cfg(unix)]
fn open_shm(path: Option<String>) -> Option<Arc<dyn BusSource>> {
    let path = path?;
    match super::shm::SharedSegment::open(std::path::Path::new(&path)) {
        Ok(seg) => {
            info!(
                "shared segment mapped at {path} ({} control buses, zero-message meters)",
                seg.control_buses()
            );
            Some(Arc::new(seg))
        }
        Err(e) => {
            warn!("cannot map shared segment {path}: {e}; meters will read zero");
            None
        }
    }
}

#[cfg(not(unix))]
fn open_shm(path: Option<String>) -> Option<Arc<dyn BusSource>> {
    if path.is_some() {
        warn!("--shm (shared-memory meters) is only supported on Unix");
    }
    None
}

/// An in-progress pointer drag, by what it is driving.
enum Drag {
    /// A slider: the value follows the cursor within `body` — along x, or along y
    /// when `vertical`.
    Slider { id: i32, body: Rect, vertical: bool },
    /// A knob or number: the value moves incrementally with the vertical drag.
    /// On press the pointer is grabbed (see [`App::grab_pointer`]) so motion does
    /// not stop over the window's title bar or past its edges, where `CursorMoved`
    /// is otherwise swallowed. `locked` records which grab won: when `true` the
    /// pointer is locked and motion arrives as relative `DeviceEvent::MouseMotion`;
    /// when `false` (confined or ungrabbed) `CursorMoved` still drives it, and
    /// `last_y` re-anchors on every step so a value pinned at an end has no dead
    /// zone — reversing direction moves it at once instead of sticking and jumping.
    Vertical {
        id: i32,
        last_y: f64,
        body_h: f32,
        locked: bool,
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
/// script address its events go to, the pointer/drag state, and the per-`scope`
/// rolling history. The widget tree itself lives in the [`Host`] (single source
/// of truth).
struct WindowState {
    gpu: Gpu,
    waveforms: HashMap<i32, WaveformSlot>,
    /// Per-`canvas` GPU resources (the compiled user shader + uniforms).
    canvases: HashMap<i32, CanvasView>,
    painter: Painter,
    origin: SocketAddr,
    cursor: (f64, f64),
    drag: Option<Drag>,
    /// Recent control-bus samples per `scope` widget id (oldest .. newest).
    scopes: HashMap<i32, VecDeque<f32>>,
    /// Triggered display window per audio-rate `scope` widget id, refreshed on
    /// the frame tick from the shared segment's tap rings. Also holds each
    /// `phasescope`'s interleaved L/R window (ids do not collide).
    tap_windows: HashMap<i32, Vec<f32>>,
    /// Persistent FFT analysis state per `spectrum` widget id (the smoothed and
    /// peak-hold curves), advanced on the frame tick.
    spectra: HashMap<i32, SpectrumState>,
}

struct App {
    host: Host,
    socket: Arc<UdpSocket>,
    /// Live control-bus source (the shared segment) for meters/scopes, if mapped.
    shm: Option<Arc<dyn BusSource>>,
    windows: HashMap<i32, WindowState>,
    by_winit: HashMap<WindowId, i32>,
    /// Window opens requested before the first `resumed`, flushed on resume.
    pending: Vec<(i32, SocketAddr)>,
    resumed: bool,
    /// Next scheduled repaint for animated (meter/scope) windows.
    next_frame: Instant,
    /// The server-buffer fetch machine (`/b_query` → chunked `/b_getn`),
    /// shared with the browser front.
    fetches: BufferFetches,
    /// The node tree last read from the server, by group id, feeding `nodetree`
    /// widgets (filled by `/g_queryTree.reply`).
    node_trees: HashMap<i32, NodeTree>,
    /// Whether the client leg has registered for node notifications
    /// (`/notify 1`), so it is sent once even with several node-tree windows.
    notified: bool,
    /// Next scheduled re-query of the server's node tree (the `/n_set` poll).
    next_query: Instant,
    /// Standalone mode: the host booted a pre-loaded GuiDef with no script front
    /// (`--standalone`). Closing the last window then quits the app, so the
    /// embedded audio server is dropped (and `/quit`ed) instead of left running.
    standalone: bool,
}

impl App {
    fn new(host: Host, socket: Arc<UdpSocket>, shm: Option<Arc<dyn BusSource>>) -> Self {
        Self {
            host,
            socket,
            shm,
            windows: HashMap::new(),
            by_winit: HashMap::new(),
            pending: Vec::new(),
            resumed: false,
            next_frame: Instant::now(),
            fetches: BufferFetches::default(),
            node_trees: HashMap::new(),
            notified: false,
            next_query: Instant::now(),
            standalone: false,
        }
    }

    /// The current value of control bus `bus` from the shared segment (`0.0`
    /// without a segment or for a negative/out-of-range bus).
    fn read_bus(&self, bus: i32) -> f32 {
        if bus < 0 {
            return 0.0;
        }
        self.shm.as_ref().map_or(0.0, |s| s.control(bus as usize))
    }

    /// Pushes one fresh sample into every `scope`'s rolling history, read from the
    /// shared segment. Called once per animation frame tick (not per `render`), so
    /// the scope scrolls at a steady, time-based rate regardless of how often the
    /// window happens to repaint.
    fn advance_scopes(&mut self) {
        // Collect (window, scope id, bus value) under immutable borrows, then push
        // — keeps the shared-segment read and the per-window history mutation
        // from overlapping borrows of `self`.
        let mut samples: Vec<(i32, i32, f32)> = Vec::new();
        for def_id in self.windows.keys() {
            if let Some(tree) = self.host.window_def(*def_id) {
                let mut scopes = Vec::new();
                collect_scopes(tree, &mut scopes);
                for (id, bus) in scopes {
                    samples.push((*def_id, id, self.read_bus(bus)));
                }
            }
        }
        for (def_id, id, value) in samples {
            if let Some(ws) = self.windows.get_mut(&def_id) {
                push_sample(ws.scopes.entry(id).or_default(), value);
            }
        }
    }

    /// Refreshes every audio-tap consumer from the shared segment's tap rings,
    /// once per animation frame tick (the same cadence as
    /// [`Self::advance_scopes`]): the audio-rate scopes' triggered windows, the
    /// phasescopes' interleaved L/R windows, and the spectra's FFT analysis.
    /// Without a segment the views stay empty and draw their framed field.
    fn advance_tap_windows(&mut self) {
        let Some(shm) = self.shm.clone() else {
            return;
        };
        let sample_rate = shm.sample_rate();
        for (def_id, ws) in &mut self.windows {
            if let Some(tree) = self.host.window_def(*def_id) {
                super::live::update_tap_windows(
                    tree,
                    sample_rate,
                    |tap, out| shm.read_tap(tap, out),
                    &mut ws.tap_windows,
                );
                super::live::update_phase_windows(
                    tree,
                    sample_rate,
                    |tap, out| shm.read_tap(tap, out),
                    &mut ws.tap_windows,
                );
                super::live::update_spectra(
                    tree,
                    |tap, out| shm.read_tap(tap, out),
                    &mut ws.spectra,
                );
            }
        }
    }

    /// Whether window `def_id` should repaint continuously: it has a `canvas`
    /// (time-driven, always), or a meter/scope with a shared segment to feed it.
    fn window_is_animated(&self, def_id: i32) -> bool {
        self.host.window_def(def_id).is_some_and(|tree| {
            tree_has_canvas(tree) || (self.shm.is_some() && tree_has_live_widget(tree))
        })
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

    /// Delivers a control's current value: straight to the audio server when the
    /// widget is bound (`/gui_bind` — no script round-trip), otherwise as a
    /// `/gui_event` to the script.
    fn emit_value(&self, def_id: i32, widget_id: i32) {
        if let Some(value) = self
            .host
            .window_def(def_id)
            .and_then(|t| value_of(t, widget_id))
        {
            self.deliver(def_id, widget_id, value);
        }
    }

    /// Routes a widget's new `value` to the audio server when it is bound
    /// (`/gui_bind`, the low-latency path that bypasses the script), or to the
    /// script as a `/gui_event` otherwise. Every interaction that produces a
    /// value goes through here, so a single binding check covers them all.
    fn deliver(&self, def_id: i32, widget_id: i32, value: OscType) {
        if self.host.forward(widget_id, value.clone()) {
            return; // bound: the value went straight to the audio server
        }
        self.emit(def_id, widget_id, vec![value]);
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
        let gpu = match pollster::block_on(Gpu::new(window)) {
            Ok(gpu) => gpu,
            Err(e) => return warn!("gui_def {id}: cannot start the GPU: {e}"),
        };

        let mut waveforms = HashMap::new();
        let mut buffer_refs = Vec::new();
        let mut canvases = HashMap::new();
        if let Some(tree) = self.host.window_def(id) {
            collect_waveforms(tree, &gpu, &mut waveforms, &mut buffer_refs);
            collect_canvases(tree, &gpu, &mut canvases);
        }
        let painter = Painter::new(&gpu.device, gpu.config.format);

        self.by_winit.insert(winit_id, id);
        self.windows.insert(
            id,
            WindowState {
                gpu,
                waveforms,
                canvases,
                painter,
                origin,
                cursor: (0.0, 0.0),
                drag: None,
                scopes: HashMap::new(),
                tap_windows: HashMap::new(),
                spectra: HashMap::new(),
            },
        );
        info!("gui_def {id}: opened window \"{title}\"");
        // Plots that name a local file map it now (the bulk path, no OSC); the
        // samples land in the host tree the renderer reads each frame.
        if let Some(root) = self.host.window_def_mut(id) {
            load_plot_paths(root);
        }
        if let Some(ws) = self.windows.get(&id) {
            ws.gpu.window.request_redraw();
        }
        // Kick off fetches for any waveform that references a server buffer.
        self.start_buffer_fetches(id, buffer_refs);
        // A node-tree view drives the client leg: register for notifications and
        // query at once, so it shows the tree without waiting for the first poll.
        if self.host.window_def(id).is_some_and(tree_has_node_tree) && self.host.server().is_some()
        {
            self.ensure_notify();
            self.requery_node_trees();
            self.next_query = Instant::now() + NODETREE_POLL;
        }
    }

    /// Registers waveform widgets that reference a server buffer and queries the
    /// audio server for each distinct buffer's shape (the fetch proceeds on the
    /// `/b_info` reply). `refs` is `(widget_id, bufnum, base_bucket)`.
    fn start_buffer_fetches(&mut self, def_id: i32, refs: Vec<(i32, i32, usize)>) {
        for (widget_id, bufnum, base_bucket) in refs {
            if let Some(query) = self.fetches.want(def_id, widget_id, bufnum, base_bucket) {
                self.send_to_server(query);
            }
        }
    }

    /// Sends one fetch-machine message over the client leg (`/b_query`,
    /// `/b_getn`), warning instead of failing when no server is attached.
    fn send_to_server(&self, msg: OscMessage) {
        let Some(server) = self.host.server() else {
            return warn!(
                "waveform references a server buffer but no audio server is attached (--server)"
            );
        };
        let addr = msg.addr.clone();
        if let Err(e) = server.send(msg) {
            warn!("failed to send {addr} to the audio server: {e}");
        }
    }

    /// Carries out one fetch-machine step: send the next request, or place a
    /// finished buffer into every window that was waiting on it.
    fn apply_fetch_step(&mut self, step: FetchStep) {
        match step {
            FetchStep::Request(msg) => self.send_to_server(msg),
            FetchStep::Done {
                bufnum,
                mono,
                wants,
            } => self.finalize_buffer(bufnum, mono, wants),
            FetchStep::None => {}
        }
    }

    /// Pops every pending reply from an embedded server and routes it, the
    /// embed counterpart of the UDP reply thread. Only built with the
    /// `standalone` feature (the only way to get an embed link); otherwise a
    /// no-op (see the stub below).
    #[cfg(feature = "standalone")]
    fn drain_embed_replies(&mut self) {
        if self.host.server().and_then(|s| s.embed()).is_none() {
            return;
        }
        let mut packets: Vec<Vec<u8>> = Vec::new();
        if let Some(embed) = self.host.server().and_then(|s| s.embed()) {
            let mut buf = vec![0u8; 65536];
            while let Some(n) = embed.poll_into(&mut buf) {
                packets.push(buf[..n].to_vec());
            }
        }
        for bytes in packets {
            match clausters_core::osc::decode_packet(&bytes) {
                Ok(packet) => self.handle_server_packet(packet),
                Err(e) => warn!("malformed OSC reply from the embedded server: {e}"),
            }
        }
    }

    /// Without the `standalone` feature there is no embed link, so draining its
    /// replies is nothing — kept so the event loop calls it unconditionally.
    #[cfg(not(feature = "standalone"))]
    fn drain_embed_replies(&mut self) {}

    /// Routes one decoded reply from the audio server (the client leg).
    fn handle_server_packet(&mut self, packet: OscPacket) {
        let OscPacket::Message(msg) = packet else {
            return; // bundles are not used on the reply path yet
        };
        match msg.addr.as_str() {
            "/b_info" => {
                // (bufnum, frames, channels, sampleRate) per buffer.
                for group in msg.args.chunks(4) {
                    if let [
                        OscType::Int(bufnum),
                        OscType::Int(frames),
                        OscType::Int(channels),
                        _,
                    ] = group
                    {
                        let step = self.fetches.on_info(
                            *bufnum,
                            (*frames).max(0) as usize,
                            (*channels).max(0) as usize,
                        );
                        self.apply_fetch_step(step);
                    }
                }
            }
            "/b_setn" => {
                let step = self.fetches.on_data(&msg.args);
                self.apply_fetch_step(step);
            }
            "/g_queryTree.reply" => self.on_query_tree_reply(&msg.args),
            // A node was created or freed (on any client): refresh the tree
            // promptly instead of waiting for the next poll.
            "/n_go" | "/n_end" => self.next_query = Instant::now(),
            "/fail" => warn!("audio server replied /fail: {:?}", msg.args),
            _ => {}
        }
    }

    /// `/g_queryTree.reply`: parse the server's node tree, store it by group and
    /// repaint the windows showing it (only when it actually changed, so an
    /// idle tree polled at a few Hz does not repaint needlessly).
    fn on_query_tree_reply(&mut self, args: &[OscType]) {
        let Some(tree) = NodeTree::parse(args) else {
            return warn!("malformed /g_queryTree.reply ({} args)", args.len());
        };
        let group = tree.group;
        if self.node_trees.get(&group) == Some(&tree) {
            return;
        }
        debug!(
            "node tree for group {group} updated ({} top-level node(s))",
            tree.root.len()
        );
        self.node_trees.insert(group, tree);
        let ids: Vec<i32> = self
            .windows
            .keys()
            .copied()
            .filter(|id| self.window_shows_group(*id, group))
            .collect();
        for id in ids {
            self.redraw(id);
        }
    }

    /// The distinct server groups any open window's `nodetree` widgets mirror.
    fn node_tree_groups(&self) -> Vec<i32> {
        let mut groups = Vec::new();
        for id in self.windows.keys() {
            if let Some(tree) = self.host.window_def(*id) {
                collect_node_tree_groups(tree, &mut groups);
            }
        }
        groups
    }

    /// Whether window `def_id` has a `nodetree` mirroring `group`.
    fn window_shows_group(&self, def_id: i32, group: i32) -> bool {
        let mut groups = Vec::new();
        if let Some(tree) = self.host.window_def(def_id) {
            collect_node_tree_groups(tree, &mut groups);
        }
        groups.contains(&group)
    }

    /// Registers for node lifecycle notifications (`/notify 1`) once, so a
    /// `nodetree` refreshes as soon as nodes appear or disappear.
    fn ensure_notify(&mut self) {
        if self.notified {
            return;
        }
        if let Some(server) = self.host.server() {
            if let Err(e) = server.send(OscMessage {
                addr: "/notify".into(),
                args: vec![OscType::Int(1)],
            }) {
                return warn!("failed to register for node notifications: {e}");
            }
            self.notified = true;
        }
    }

    /// Sends a `/g_queryTree <group> 1` for every group an open `nodetree` shows.
    fn requery_node_trees(&self) {
        let Some(server) = self.host.server() else {
            return;
        };
        for group in self.node_tree_groups() {
            if let Err(e) = server.send(OscMessage {
                addr: "/g_queryTree".into(),
                args: vec![OscType::Int(group), OscType::Int(1)],
            }) {
                warn!("failed to query node tree for group {group}: {e}");
            }
        }
    }

    /// A buffer finished downloading (channel 0 already de-interleaved by the
    /// fetch machine): build a waveform view in each window that waited on it.
    fn finalize_buffer(&mut self, bufnum: i32, mono: Arc<[f32]>, wants: Vec<WaveWant>) {
        info!(
            "buffer {bufnum}: {} frames loaded into {} waveform(s)",
            mono.len(),
            wants.len()
        );
        for want in wants {
            if let Some(ws) = self.windows.get_mut(&want.def_id) {
                let data = WaveformData::new(Arc::clone(&mono), want.base_bucket);
                let slot = frame::waveform_slot(data, &ws.gpu);
                ws.waveforms.insert(want.widget_id, slot);
                ws.gpu.window.request_redraw();
            }
        }
    }

    fn drop_window(&mut self, id: i32) {
        if let Some(ws) = self.windows.remove(&id) {
            self.by_winit.remove(&ws.gpu.window.id());
        }
        // Drop any pending buffer wants this window had, so a finished fetch does
        // not try to fill a window that is gone (or being rebuilt).
        self.fetches.drop_def(id);
    }

    /// User-initiated close: tell the script, then drop the window. A standalone
    /// window has the placeholder origin (port 0) — there is no script to notify,
    /// so the `/gui_closed` is skipped (sending to port 0 fails with EINVAL).
    fn close_by_user(&mut self, id: i32) {
        if let Some(ws) = self.windows.get(&id)
            && ws.origin.port() != 0
        {
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

    /// Closes a window on user request, and quits the app once the last window is
    /// gone in standalone mode — so the embedded audio server is dropped (and
    /// `/quit`ed) rather than left running with no window. A script-driven host
    /// stays alive (the script may open another window); only standalone exits.
    fn user_close(&mut self, id: i32, event_loop: &ActiveEventLoop) {
        self.close_by_user(id);
        if self.standalone && self.windows.is_empty() {
            event_loop.exit();
        }
    }

    /// The framebuffer size of a window.
    fn fb(&self, def_id: i32) -> (u32, u32) {
        self.windows
            .get(&def_id)
            .map(|w| (w.gpu.config.width.max(1), w.gpu.config.height.max(1)))
            .unwrap_or((1, 1))
    }

    /// The deepest widget under `(x, y)`: its id, rect and a clone of its kind
    /// (the shared hit-test over the host tree).
    fn hit(&self, def_id: i32, x: f64, y: f64) -> Option<(i32, Rect, WidgetKind)> {
        let (fb_w, fb_h) = self.fb(def_id);
        interact::hit(&self.host, def_id, fb_w, fb_h, x, y)
    }

    /// The current 0..1 fraction of a continuous control (slider/knob/number) in
    /// the host tree — the live value used to drive an incremental drag.
    fn fraction_of(&self, def_id: i32, widget_id: i32) -> Option<f32> {
        interact::fraction_of(&self.host, def_id, widget_id)
    }

    /// Sets a continuous control's value from a 0..1 fraction, in the host tree.
    fn set_fraction(&mut self, def_id: i32, widget_id: i32, t: f32) {
        interact::set_fraction(&mut self.host, def_id, widget_id, t);
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
            WidgetKind::Slider { range: r, vertical } => {
                let body = controls::body_rect(rect, r.label.is_some());
                let t = slider_t(body, cx, cy, vertical);
                self.set_fraction(def_id, id, t);
                self.emit_value(def_id, id);
                self.set_drag(def_id, Drag::Slider { id, body, vertical });
                self.redraw(def_id);
            }
            WidgetKind::Knob(r) | WidgetKind::Number(r) => {
                let body = controls::body_rect(rect, r.label.is_some());
                let locked = self.grab_pointer(def_id);
                self.set_drag(
                    def_id,
                    Drag::Vertical {
                        id,
                        last_y: cy,
                        body_h: body.h,
                        locked,
                    },
                );
            }
            WidgetKind::Button { .. } => {
                self.deliver(def_id, id, OscType::Int(1));
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

    /// Grabs the pointer for a knob/number drag so motion keeps arriving even
    /// over the window decorations or past its edges, where `CursorMoved`
    /// otherwise stops (the title-bar/out-of-surface gap). Tries `Locked` first —
    /// the cursor stays put and motion comes as relative `DeviceEvent::MouseMotion`
    /// (the canonical knob feel, unbounded range) — and falls back to `Confined`,
    /// which keeps the cursor inside the client area (so it cannot reach the title
    /// bar) and is still driven by `CursorMoved`. Returns whether the pointer was
    /// *locked* (which motion source the drag should read).
    fn grab_pointer(&self, def_id: i32) -> bool {
        let Some(ws) = self.windows.get(&def_id) else {
            return false;
        };
        let window = &ws.gpu.window;
        if window.set_cursor_grab(CursorGrabMode::Locked).is_ok() {
            window.set_cursor_visible(false);
            return true;
        }
        if let Err(e) = window.set_cursor_grab(CursorGrabMode::Confined) {
            debug!("gui_def {def_id}: no pointer grab for the drag ({e})");
        }
        false
    }

    /// Releases the pointer grab a knob/number drag took and restores the cursor.
    fn release_pointer(&self, def_id: i32) {
        if let Some(ws) = self.windows.get(&def_id) {
            let _ = ws.gpu.window.set_cursor_grab(CursorGrabMode::None);
            ws.gpu.window.set_cursor_visible(true);
        }
    }

    fn flip_toggle(&mut self, def_id: i32, id: i32) {
        interact::flip_toggle(&mut self.host, def_id, id);
    }

    fn cycle_menu(&mut self, def_id: i32, id: i32) {
        interact::cycle_menu(&mut self.host, def_id, id);
    }

    /// Pointer moved while a drag is active: drive the dragged target.
    fn on_drag(&mut self, def_id: i32, cx: f64, cy: f64) {
        // Read the drag descriptor out (cheap copies) to release the borrow.
        let action = self
            .windows
            .get(&def_id)
            .and_then(|w| w.drag.as_ref())
            .map(|d| match d {
                Drag::Slider { id, body, vertical } => DragMove::Slider(*id, *body, *vertical),
                // A locked drag is driven by relative motion in `device_event`,
                // not by these cursor positions, so skip it here.
                Drag::Vertical {
                    id,
                    last_y,
                    body_h,
                    locked,
                } => {
                    if *locked {
                        DragMove::None
                    } else {
                        DragMove::Vertical(*id, *last_y, *body_h)
                    }
                }
                Drag::Button { .. } => DragMove::None,
                Drag::Waveform {
                    id,
                    origin_x,
                    start,
                    body_w,
                } => DragMove::Waveform(*id, *origin_x, *start, *body_w),
            });
        match action {
            Some(DragMove::Slider(id, body, vertical)) => {
                let t = slider_t(body, cx, cy, vertical);
                self.set_fraction(def_id, id, t);
                self.emit_value(def_id, id);
                self.redraw(def_id);
            }
            Some(DragMove::Vertical(id, last_y, body_h)) => {
                // Incremental: add this step's delta to the *current* (clamped)
                // fraction and re-anchor `last_y`. A value pinned at an end stays
                // put, but reversing moves it immediately — no snapshot dead zone.
                let cur = self.fraction_of(def_id, id).unwrap_or(0.0);
                let t = (cur + controls::drag_fraction_delta(cy - last_y, body_h)).clamp(0.0, 1.0);
                self.set_fraction(def_id, id, t);
                if let Some(Drag::Vertical { last_y, .. }) =
                    self.windows.get_mut(&def_id).and_then(|w| w.drag.as_mut())
                {
                    *last_y = cy;
                }
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

    /// Release: a held button emits 0; a knob/number drag releases its pointer
    /// grab; any drag ends.
    fn on_release(&mut self, def_id: i32) {
        let drag = self.windows.get_mut(&def_id).and_then(|w| w.drag.take());
        match drag {
            Some(Drag::Button { id }) => {
                self.deliver(def_id, id, OscType::Int(0));
                self.redraw(def_id);
            }
            Some(Drag::Vertical { .. }) => self.release_pointer(def_id),
            _ => {}
        }
    }

    /// Renders window `def_id` through the shared frame path ([`frame::render`]),
    /// the same code the browser front drives — here fed the live inputs (the
    /// shared-memory bus, the scope histories, the node trees, the held button).
    fn render(&mut self, def_id: i32) {
        let active_button = match self.windows.get(&def_id).and_then(|w| w.drag.as_ref()) {
            Some(Drag::Button { id }) => Some(*id),
            _ => None,
        };
        let server_attached = self.host.server().is_some();
        // Disjoint field borrows: the tree (host), the bus (shm), the node trees,
        // and the window's GPU resources are separate fields of `self`.
        let Some(tree) = self.host.window_def(def_id) else {
            return;
        };
        let inputs = frame::FrameInputs {
            bus: self.shm.as_deref(),
            node_trees: &self.node_trees,
            active_button,
            server_attached,
            sample_rate: self.shm.as_ref().map_or(0.0, |s| s.sample_rate()),
        };
        let Some(ws) = self.windows.get_mut(&def_id) else {
            return;
        };
        frame::render(
            &mut ws.gpu,
            &mut ws.painter,
            &mut ws.waveforms,
            &mut ws.canvases,
            &ws.scopes,
            &ws.tap_windows,
            &ws.spectra,
            tree,
            &inputs,
        );
    }
}

/// A drag step, copied out of the borrow so the host tree can be mutated.
enum DragMove {
    Slider(i32, Rect, bool),
    Vertical(i32, f64, f32),
    Waveform(i32, f64, f64, f64),
    None,
}

/// Walks the tree building waveform views. A `cache`/`path` waveform is loaded
/// **now** from a mapped local resource (the G7 bulk path, no OSC); a
/// server-`buffer` reference with no data is deferred as a
/// `(widget_id, bufnum, base_bucket)` entry in `buffer_refs` for the client leg
/// to fetch; inline/blob (and empty) samples build a slot directly.
fn collect_waveforms(
    widget: &Widget,
    gpu: &Gpu,
    out: &mut HashMap<i32, WaveformSlot>,
    buffer_refs: &mut Vec<(i32, i32, usize)>,
) {
    if let WidgetKind::Waveform {
        samples,
        base_bucket,
        buffer,
        path,
        cache,
        channels,
    } = &widget.kind
        && let Some(id) = widget.id
    {
        if cache.is_some() || path.is_some() {
            // Bulk path: map a local resource (raw samples or a prebuilt cache)
            // through the BulkLoader seam, then build the GPU slot from the data.
            if let Some(data) =
                MmapLoader.waveform(cache.as_deref(), path.as_deref(), *channels, *base_bucket)
            {
                out.insert(id, frame::waveform_slot(data, gpu));
            }
        } else if let (Some(bufnum), true) = (buffer, samples.is_empty()) {
            // A server buffer with no inline data: fetch it over the client leg.
            buffer_refs.push((id, *bufnum, *base_bucket));
        } else {
            out.insert(
                id,
                frame::waveform_slot(WaveformData::new(Arc::clone(samples), *base_bucket), gpu),
            );
        }
    }
    for child in &widget.children {
        collect_waveforms(child, gpu, out, buffer_refs);
    }
}

/// Builds a [`CanvasView`] (compiling the user shader) for every `canvas` in the
/// tree, keyed by widget id.
fn collect_canvases(widget: &Widget, gpu: &Gpu, out: &mut HashMap<i32, CanvasView>) {
    if let WidgetKind::Canvas { shader, .. } = &widget.kind
        && let Some(id) = widget.id
    {
        out.insert(id, CanvasView::new(&gpu.device, gpu.config.format, shader));
    }
    for child in &widget.children {
        collect_canvases(child, gpu, out);
    }
}

/// Whether a widget tree contains a `nodetree` view (so the window drives the
/// node-tree query/notify path).
fn tree_has_node_tree(widget: &Widget) -> bool {
    widget.kind.node_tree_group().is_some() || widget.children.iter().any(tree_has_node_tree)
}

/// Appends the distinct server groups every `nodetree` in `widget` mirrors.
fn collect_node_tree_groups(widget: &Widget, out: &mut Vec<i32>) {
    if let Some(group) = widget.kind.node_tree_group()
        && !out.contains(&group)
    {
        out.push(group);
    }
    for child in &widget.children {
        collect_node_tree_groups(child, out);
    }
}

/// Maps a `plot`'s local resource into its tree node: a `path` of raw
/// little-endian `f32` mapped read-only and de-interleaved to channel 0 (the
/// bulk path, no OSC). Walks children too. Already-loaded (inline) plots and
/// plots without a path are left as they are.
fn load_plot_paths(widget: &mut Widget) {
    if let WidgetKind::Plot {
        samples,
        path,
        channels,
        ..
    } = &mut widget.kind
        && samples.is_empty()
        && let Some(p) = path.clone()
        && let Some(loaded) = MmapLoader.plot_samples(&p, *channels)
    {
        *samples = loaded;
    }
    for child in &mut widget.children {
        load_plot_paths(child);
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.resumed = true;
        // Deliver raw motion while focused, so a locked knob/number drag reads its
        // relative `DeviceEvent::MouseMotion` (the pointer-lock path in `on_press`).
        event_loop.listen_device_events(DeviceEvents::WhenFocused);
        for (id, origin) in std::mem::take(&mut self.pending) {
            self.open_window(event_loop, id, origin);
        }
        // Standalone: a GuiDef pre-loaded into the host before the loop started
        // (no `/gui_def` over the wire) is opened now. Its events have no script
        // to return to, so they go to a placeholder origin. Pre-loaded windows
        // mean this is a standalone app — closing the last one quits it.
        let standalone_origin = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        let preloaded = self.host.window_def_ids();
        self.standalone = !preloaded.is_empty();
        for id in preloaded {
            if !self.windows.contains_key(&id) {
                self.open_window(event_loop, id, standalone_origin);
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Osc { from, bytes } => {
                let packet = match clausters_core::osc::decode_packet(&bytes) {
                    Ok(p) => p,
                    Err(e) => return warn!("malformed OSC packet from {from}: {e}"),
                };
                let effects = self.host.handle_packet(packet, ClientId::Udp(from));
                self.apply(event_loop, from, effects);
            }
            UserEvent::ServerOsc { bytes } => match clausters_core::osc::decode_packet(&bytes) {
                Ok(packet) => self.handle_server_packet(packet),
                Err(e) => warn!("malformed OSC reply from the audio server: {e}"),
            },
        }
    }

    /// Raw relative motion drives a *locked* knob/number drag. The pointer is
    /// locked in place (so it cannot wander onto the title bar or out of the
    /// window, where `CursorMoved` is lost), and its movement arrives here as a
    /// device delta instead — applied incrementally to the dragged control.
    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        let DeviceEvent::MouseMotion { delta: (_, dy) } = event else {
            return;
        };
        // The window (if any) whose active drag is a locked knob/number. Only one
        // pointer drag runs at a time, so the first match is the target.
        let Some((def_id, id, body_h)) =
            self.windows.iter().find_map(|(def_id, ws)| match &ws.drag {
                Some(Drag::Vertical {
                    id,
                    body_h,
                    locked: true,
                    ..
                }) => Some((*def_id, *id, *body_h)),
                _ => None,
            })
        else {
            return;
        };
        let cur = self.fraction_of(def_id, id).unwrap_or(0.0);
        let t = (cur + controls::drag_fraction_delta(dy, body_h)).clamp(0.0, 1.0);
        self.set_fraction(def_id, id, t);
        self.emit_value(def_id, id);
        self.redraw(def_id);
    }

    /// After handling events, schedule the next wake-up: a ~30 fps repaint for
    /// animated (meter/scope) windows so their shared-memory values keep moving,
    /// and a low-rate re-query for node-tree windows so `/n_set` changes show.
    /// With neither, windows stay event-driven (`Wait`).
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let mut next_wake: Option<Instant> = None;

        // Drain replies from an embedded server (standalone): the UDP leg uses a
        // background thread, but the embed ring is polled here on the main thread.
        self.drain_embed_replies();

        // Meter/scope animation, driven from the shared segment.
        let animated: Vec<i32> = self
            .windows
            .keys()
            .copied()
            .filter(|id| self.window_is_animated(*id))
            .collect();
        if !animated.is_empty() {
            if now >= self.next_frame {
                // Advance each scope's rolling history exactly once per frame tick
                // (time-based), then repaint. Sampling here rather than in `render`
                // keeps the scroll speed constant: extra repaints from a drag or a
                // resize no longer push extra samples and speed the scope up. The
                // audio-rate scopes refresh their triggered tap windows likewise.
                self.advance_scopes();
                self.advance_tap_windows();
                for id in &animated {
                    if let Some(ws) = self.windows.get(id) {
                        ws.gpu.window.request_redraw();
                    }
                }
                self.next_frame = now + FRAME;
            }
            next_wake = Some(self.next_frame);
        }

        // Node-tree polling, driven from the client leg (the `/n_set` poll).
        if self.host.server().is_some() && !self.node_tree_groups().is_empty() {
            if now >= self.next_query {
                self.requery_node_trees();
                self.next_query = now + NODETREE_POLL;
            }
            next_wake = Some(next_wake.map_or(self.next_query, |t| t.min(self.next_query)));
        }

        match next_wake {
            Some(t) => event_loop.set_control_flow(ControlFlow::WaitUntil(t)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(&def_id) = self.by_winit.get(&window_id) else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => self.user_close(def_id, event_loop),
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
                    Key::Named(NamedKey::Escape) => self.user_close(def_id, event_loop),
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
