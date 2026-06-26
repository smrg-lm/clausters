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
use std::net::{SocketAddr, UdpSocket};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clausters_core::osc::{OscMessage, OscPacket, OscType, encode};
use tracing::{debug, info, warn};
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
use super::nodetree::{self, NodeTree};
use super::paint::{Color, Mesh, Painter};
use super::widget::{Widget, WidgetKind};
use super::{BusSource, ClientId, GUI_CLOSED, GUI_EVENT, Host, HostEffect, controls, meters, plot};

/// Repaint period for windows with live (shared-memory-backed) widgets — ~30 fps,
/// enough for smooth meters/scopes without spinning the CPU.
const FRAME: Duration = Duration::from_millis(33);
/// Most recent control-bus samples a `scope` keeps and plots.
const SCOPE_HISTORY: usize = 512;
/// Samples per `/b_getn` request when pulling a server buffer (each reply must
/// fit a datagram; the bulk-transfer optimization is a later milestone).
const BUFFER_CHUNK: usize = 8192;
/// How often a window with a `nodetree` re-queries the server's tree. Node
/// creation/removal is caught immediately through `/n_go`/`/n_end`; this low-rate
/// poll picks up `/n_set` control changes (which raise no notification).
const NODETREE_POLL: Duration = Duration::from_millis(200);

const CLEAR: wgpu::Color = wgpu::Color {
    r: 0.05,
    g: 0.05,
    b: 0.07,
    a: 1.0,
};
const PANEL_COLOR: Color = [0.10, 0.11, 0.14, 0.55];
const LABEL_COLOR: Color = [0.85, 0.87, 0.90, 1.0];
const LABEL_SCALE: f32 = 2.0;

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
    // The host <- audio-server reply path (only when a client leg is attached).
    if let Some(leg_socket) = host.server().map(|s| s.socket()) {
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

/// A waveform widget's GPU view plus its own navigation window.
struct WaveformSlot {
    view: WaveformView,
    nav: View,
}

/// A placed `plot` widget and the data its (static) draw needs, copied out of
/// the host tree so the mesh is built after the tree borrow is released.
struct PlotItem {
    rect: Rect,
    samples: Arc<[f32]>,
    min: f32,
    max: f32,
    label: Option<String>,
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
/// script address its events go to, the pointer/drag state, and the per-`scope`
/// rolling history. The widget tree itself lives in the [`Host`] (single source
/// of truth).
struct WindowState {
    gpu: Gpu,
    waveforms: HashMap<i32, WaveformSlot>,
    painter: Painter,
    origin: SocketAddr,
    cursor: (f64, f64),
    drag: Option<Drag>,
    /// Recent control-bus samples per `scope` widget id (oldest .. newest).
    scopes: HashMap<i32, VecDeque<f32>>,
}

/// A waveform widget waiting on a server buffer fetch.
struct WaveWant {
    def_id: i32,
    widget_id: i32,
    base_bucket: usize,
}

/// An in-progress fetch of a server buffer over the client leg: the flat
/// interleaved samples filled in as `/b_setn` chunks arrive.
struct BufferFetch {
    channels: usize,
    total: usize,
    samples: Vec<f32>,
    received: usize,
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
    /// Waveform widgets awaiting each server buffer number.
    wants: HashMap<i32, Vec<WaveWant>>,
    /// In-progress server-buffer fetches, by buffer number.
    fetches: HashMap<i32, BufferFetch>,
    /// The node tree last read from the server, by group id, feeding `nodetree`
    /// widgets (filled by `/g_queryTree.reply`).
    node_trees: HashMap<i32, NodeTree>,
    /// Whether the client leg has registered for node notifications
    /// (`/notify 1`), so it is sent once even with several node-tree windows.
    notified: bool,
    /// Next scheduled re-query of the server's node tree (the `/n_set` poll).
    next_query: Instant,
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
            wants: HashMap::new(),
            fetches: HashMap::new(),
            node_trees: HashMap::new(),
            notified: false,
            next_query: Instant::now(),
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

    /// Whether window `def_id` should repaint continuously: it has a meter/scope
    /// and there is a shared segment to feed it.
    fn window_is_animated(&self, def_id: i32) -> bool {
        self.shm.is_some()
            && self
                .host
                .window_def(def_id)
                .is_some_and(tree_has_live_widget)
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
        let gpu = pollster::block_on(Gpu::new(window));

        let mut waveforms = HashMap::new();
        let mut buffer_refs = Vec::new();
        if let Some(tree) = self.host.window_def(id) {
            collect_waveforms(tree, &gpu, &mut waveforms, &mut buffer_refs);
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
                scopes: HashMap::new(),
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
            let first = !self.wants.contains_key(&bufnum);
            self.wants.entry(bufnum).or_default().push(WaveWant {
                def_id,
                widget_id,
                base_bucket,
            });
            if first && !self.fetches.contains_key(&bufnum) {
                self.query_buffer(bufnum);
            }
        }
    }

    /// Asks the audio server for buffer `bufnum`'s shape (`/b_query` -> `/b_info`).
    fn query_buffer(&self, bufnum: i32) {
        let Some(server) = self.host.server() else {
            return warn!(
                "waveform references buffer {bufnum} but no audio server is attached (--server)"
            );
        };
        if let Err(e) = server.send(OscMessage {
            addr: "/b_query".into(),
            args: vec![OscType::Int(bufnum)],
        }) {
            warn!("failed to query buffer {bufnum}: {e}");
        }
    }

    /// Requests the next sample range of `bufnum` starting at `start`.
    fn request_chunk(&self, bufnum: i32, start: usize, total: usize) {
        let count = BUFFER_CHUNK.min(total.saturating_sub(start));
        if count == 0 {
            return;
        }
        if let Some(server) = self.host.server()
            && let Err(e) = server.send(OscMessage {
                addr: "/b_getn".into(),
                args: vec![
                    OscType::Int(bufnum),
                    OscType::Int(start as i32),
                    OscType::Int(count as i32),
                ],
            })
        {
            warn!("failed to read buffer {bufnum} at {start}: {e}");
        }
    }

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
                        self.on_buffer_info(
                            *bufnum,
                            (*frames).max(0) as usize,
                            (*channels).max(0) as usize,
                        );
                    }
                }
            }
            "/b_setn" => self.on_buffer_data(&msg.args),
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

    /// `/b_info`: start fetching a buffer we are waiting on (or finalize empty
    /// when it is unallocated).
    fn on_buffer_info(&mut self, bufnum: i32, frames: usize, channels: usize) {
        if !self.wants.contains_key(&bufnum) || self.fetches.contains_key(&bufnum) {
            return;
        }
        let channels = channels.max(1);
        let total = frames * channels;
        if total == 0 {
            return self.finalize_buffer(bufnum, Vec::new(), channels);
        }
        self.fetches.insert(
            bufnum,
            BufferFetch {
                channels,
                total,
                samples: vec![0.0; total],
                received: 0,
            },
        );
        self.request_chunk(bufnum, 0, total);
    }

    /// `/b_setn bufnum start count value...`: store a chunk, then request the
    /// next one or finalize when the whole buffer has arrived.
    fn on_buffer_data(&mut self, args: &[OscType]) {
        let [
            OscType::Int(bufnum),
            OscType::Int(start),
            OscType::Int(count),
            rest @ ..,
        ] = args
        else {
            return;
        };
        let (bufnum, start) = (*bufnum, (*start).max(0) as usize);
        let count = (*count).max(0) as usize;
        let (done, total) = {
            let Some(fetch) = self.fetches.get_mut(&bufnum) else {
                return;
            };
            let end = start.saturating_add(count).min(fetch.total);
            let n = end.saturating_sub(start);
            for (i, arg) in rest.iter().take(n).enumerate() {
                if let OscType::Float(v) = arg {
                    fetch.samples[start + i] = *v;
                }
            }
            fetch.received += n;
            (fetch.received >= fetch.total, fetch.total)
        };
        if done {
            let fetch = self.fetches.remove(&bufnum).unwrap();
            self.finalize_buffer(bufnum, fetch.samples, fetch.channels);
        } else {
            self.request_chunk(bufnum, start + count, total);
        }
    }

    /// A buffer finished downloading: de-interleave channel 0 and build a
    /// waveform view in each window that was waiting on it.
    fn finalize_buffer(&mut self, bufnum: i32, interleaved: Vec<f32>, channels: usize) {
        let mono: Arc<[f32]> = if channels <= 1 {
            interleaved.into()
        } else {
            interleaved.iter().step_by(channels).copied().collect()
        };
        let wants = self.wants.remove(&bufnum).unwrap_or_default();
        info!(
            "buffer {bufnum}: {} frames loaded into {} waveform(s)",
            mono.len(),
            wants.len()
        );
        for want in wants {
            if let Some(ws) = self.windows.get_mut(&want.def_id) {
                let data = WaveformData::new(Arc::clone(&mono), want.base_bucket);
                let nav = View::full(data.total_samples());
                let view = WaveformView::new(&ws.gpu.device, ws.gpu.config.format, data);
                ws.waveforms
                    .insert(want.widget_id, WaveformSlot { view, nav });
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
        for wants in self.wants.values_mut() {
            wants.retain(|w| w.def_id != id);
        }
        self.wants.retain(|_, wants| !wants.is_empty());
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
            self.deliver(def_id, id, OscType::Int(0));
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
        // Meter/scope rects, copied out so their shared-memory values and the
        // scope history can be read after the host-tree borrow is released.
        let mut meter_rects: Vec<(Rect, i32, f32, f32, Option<String>)> = Vec::new();
        let mut scope_rects: Vec<(i32, Rect, i32, f32, f32, Option<String>)> = Vec::new();
        // Plot items (with a cheap Arc clone of the samples) and node-tree rects,
        // likewise copied out so the host-tree borrow can be released before the
        // node-tree models and the GPU resources are read.
        let mut plot_rects: Vec<PlotItem> = Vec::new();
        let mut nodetree_rects: Vec<(Rect, i32, bool, Option<String>)> = Vec::new();
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
                WidgetKind::Meter {
                    bus,
                    min,
                    max,
                    label,
                } => meter_rects.push((p.rect, *bus, *min, *max, label.clone())),
                WidgetKind::Scope {
                    bus,
                    min,
                    max,
                    label,
                } => {
                    if let Some(id) = p.widget.id {
                        scope_rects.push((id, p.rect, *bus, *min, *max, label.clone()));
                    }
                }
                WidgetKind::Plot {
                    samples,
                    min,
                    max,
                    label,
                    ..
                } => plot_rects.push(PlotItem {
                    rect: p.rect,
                    samples: Arc::clone(samples),
                    min: *min,
                    max: *max,
                    label: label.clone(),
                }),
                WidgetKind::NodeTree {
                    group,
                    controls,
                    label,
                } => nodetree_rects.push((p.rect, *group, *controls, label.clone())),
                WidgetKind::Window { .. } | WidgetKind::Unknown(_) => {}
                kind => controls::draw(&mut mesh, kind, p.rect, p.widget.id == active_button),
            }
        }

        // Meters and scopes read their control bus straight from shared memory
        // each frame (zero messages); the scope keeps a per-widget rolling
        // history in this window's state.
        for (rect, bus, min, max, label) in &meter_rects {
            let value = self.read_bus(*bus);
            let frac = meters::fraction(value, *min, *max);
            meters::draw_meter(&mut mesh, *rect, value, frac, label.as_deref());
        }
        for (id, rect, bus, min, max, label) in &scope_rects {
            let value = self.read_bus(*bus);
            if let Some(ws) = self.windows.get_mut(&def_id) {
                let history = ws.scopes.entry(*id).or_default();
                history.push_back(value);
                while history.len() > SCOPE_HISTORY {
                    history.pop_front();
                }
                let samples: Vec<f32> = history.iter().copied().collect();
                meters::draw_scope(&mut mesh, *rect, &samples, *min, *max, label.as_deref());
            }
        }

        // Static plots draw from their (already mapped) samples; node trees draw
        // from the model last read off the client leg. Both are pure mesh work
        // with the host-tree borrow already released.
        for item in &plot_rects {
            plot::draw(
                &mut mesh,
                item.rect,
                &item.samples,
                item.min,
                item.max,
                item.label.as_deref(),
            );
        }
        let server_attached = self.host.server().is_some();
        for (rect, group, controls, label) in &nodetree_rects {
            nodetree::draw(
                &mut mesh,
                *rect,
                self.node_trees.get(group),
                *controls,
                label.as_deref(),
                server_attached,
            );
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
            // Bulk path: map a local resource (raw samples or a prebuilt cache).
            if let Some(slot) = mapped_waveform_slot(
                cache.as_deref(),
                path.as_deref(),
                *channels,
                *base_bucket,
                gpu,
            ) {
                out.insert(id, slot);
            }
        } else if let (Some(bufnum), true) = (buffer, samples.is_empty()) {
            // A server buffer with no inline data: fetch it over the client leg.
            buffer_refs.push((id, *bufnum, *base_bucket));
        } else {
            out.insert(
                id,
                waveform_slot(WaveformData::new(Arc::clone(samples), *base_bucket), gpu),
            );
        }
    }
    for child in &widget.children {
        collect_waveforms(child, gpu, out, buffer_refs);
    }
}

/// A `WaveformSlot` (GPU view + a fresh full-range nav) for ready data.
fn waveform_slot(data: WaveformData, gpu: &Gpu) -> WaveformSlot {
    let nav = View::full(data.total_samples());
    let view = WaveformView::new(&gpu.device, gpu.config.format, data);
    WaveformSlot { view, nav }
}

/// Loads a waveform from a mapped local resource — the G7 bulk path that keeps a
/// multi-megabyte buffer off OSC. `cache` is a prebuilt peak-pyramid file mapped
/// and used directly (raw samples never loaded); `path` is a file of raw
/// little-endian `f32` mapped and de-interleaved (channel 0 of `channels`),
/// whose pyramid is built once and cached as a sibling `<path>.<base_bucket>.peaks`
/// so a re-open skips the rebuild. Unix-only; returns `None` (with a warning) on
/// a non-Unix host or an I/O/format error.
#[cfg(unix)]
fn mapped_waveform_slot(
    cache: Option<&Path>,
    path: Option<&Path>,
    channels: usize,
    base_bucket: usize,
    gpu: &Gpu,
) -> Option<WaveformSlot> {
    use super::mapfile::MappedFile;
    use crate::peaks::Pyramid;

    if let Some(cache) = cache {
        let map = MappedFile::open(cache)
            .map_err(|e| warn!("waveform cache {}: {e}", cache.display()))
            .ok()?;
        let pyramid = Pyramid::from_bytes(map.bytes()).or_else(|| {
            warn!("waveform cache {}: malformed peak pyramid", cache.display());
            None
        })?;
        info!(
            "waveform: mapped peak cache {} ({} samples, no raw data, no OSC)",
            cache.display(),
            pyramid.total_samples()
        );
        let data = WaveformData::with_pyramid(Arc::from([] as [f32; 0]), pyramid);
        return Some(waveform_slot(data, gpu));
    }

    let path = path?;
    let map = MappedFile::open(path)
        .map_err(|e| warn!("waveform path {}: {e}", path.display()))
        .ok()?;
    let samples: Arc<[f32]> = map.channel0_f32(channels).into();
    // Reuse a sibling cache keyed by base_bucket if it matches, else build it.
    let sibling = path.with_extension(format!("{base_bucket}.peaks"));
    let data = match Pyramid::read_cache(&sibling) {
        Ok(Some(p)) if p.total_samples() == samples.len() && p.base_bucket() == base_bucket => {
            WaveformData::with_pyramid(samples, p)
        }
        _ => {
            let data = WaveformData::new(Arc::clone(&samples), base_bucket);
            let _ = data.pyramid().write_cache(&sibling);
            data
        }
    };
    info!(
        "waveform: mapped {} samples from {} (no OSC, no re-send)",
        data.total_samples(),
        path.display()
    );
    Some(waveform_slot(data, gpu))
}

#[cfg(not(unix))]
fn mapped_waveform_slot(
    _cache: Option<&Path>,
    _path: Option<&Path>,
    _channels: usize,
    _base_bucket: usize,
    _gpu: &Gpu,
) -> Option<WaveformSlot> {
    warn!("waveform path/cache (mapped local resource) is only supported on Unix");
    None
}

/// Whether a widget tree contains a live (shared-memory-backed) meter or scope.
fn tree_has_live_widget(widget: &Widget) -> bool {
    widget.kind.live_bus().is_some() || widget.children.iter().any(tree_has_live_widget)
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
        && let Some(loaded) = map_plot_samples(&p, *channels)
    {
        *samples = loaded;
    }
    for child in &mut widget.children {
        load_plot_paths(child);
    }
}

/// Reads `path` as raw little-endian `f32`, de-interleaving channel 0 of
/// `channels` — the same read-only `mmap` the waveform bulk path uses. Unix-only;
/// returns `None` (with a warning) elsewhere or on an I/O error.
#[cfg(unix)]
fn map_plot_samples(path: &Path, channels: usize) -> Option<Arc<[f32]>> {
    use super::mapfile::MappedFile;
    let map = MappedFile::open(path)
        .map_err(|e| warn!("plot path {}: {e}", path.display()))
        .ok()?;
    let samples: Arc<[f32]> = map.channel0_f32(channels).into();
    info!(
        "plot: mapped {} samples from {} (no OSC)",
        samples.len(),
        path.display()
    );
    Some(samples)
}

#[cfg(not(unix))]
fn map_plot_samples(_path: &Path, _channels: usize) -> Option<Arc<[f32]>> {
    warn!("plot path (mapped local resource) is only supported on Unix");
    None
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

    /// After handling events, schedule the next wake-up: a ~30 fps repaint for
    /// animated (meter/scope) windows so their shared-memory values keep moving,
    /// and a low-rate re-query for node-tree windows so `/n_set` changes show.
    /// With neither, windows stay event-driven (`Wait`).
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let mut next_wake: Option<Instant> = None;

        // Meter/scope animation, driven from the shared segment.
        let animated: Vec<i32> = self
            .windows
            .keys()
            .copied()
            .filter(|id| self.window_is_animated(*id))
            .collect();
        if !animated.is_empty() {
            if now >= self.next_frame {
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
