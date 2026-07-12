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
use crate::spectrogram::Stft;
use crate::view::TimelineView;
use crate::viewport::View;
use crate::waveform::WaveformData;

use super::bulk::MmapLoader;
use super::canvas::CanvasView;
use super::fetch::{BufferFetches, FetchStep, WaveWant};
use super::frame::{self, SpectrogramSlot, WaveformSlot};
use super::graph;
use super::interact::{self, slider_t, value_of};
use super::layout::Rect;
use super::live::{collect_scopes, push_sample, tree_has_canvas, tree_has_live_widget};
use super::nodetree::NodeTree;
use super::paint::Painter;
use super::pianoroll;
use super::spectrum::SpectrumState;
use super::track;
use super::widget::{Ruler, RulerY, Widget, WidgetKind};
use super::{
    BulkLoader, BusSource, ClientId, GUI_CLOSED, GUI_EVENT, Host, HostEffect, bpf, controls,
};

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
    /// Panning a timeline view's (waveform/spectrogram) window from a snapshot
    /// (Shift+drag).
    Pan {
        id: i32,
        origin_x: f64,
        start: f64,
        body_w: f64,
    },
    /// Dragging a selection on a timeline view: `anchor` is the sample under
    /// the press; the selection spans from it to the cursor's sample.
    Select {
        id: i32,
        body_x: f64,
        body_w: f64,
        anchor: f64,
    },
    /// Panning a timeline view's **vertical** display window from a drag on
    /// its y-ruler strip: `y_start` is the window snapshot at the press,
    /// `lane_h` the lane height in device pixels (absolute panning, so a
    /// clamped edge never drifts).
    PanY {
        id: i32,
        origin_y: f64,
        y_start: f64,
        lane_h: f64,
    },
    /// Dragging a `bpf` breakpoint: the point follows the cursor within
    /// `body`, times clamped monotonic between its neighbors.
    BpfPoint { id: i32, index: usize, body: Rect },
    /// Dragging a `bpf` segment vertically: its curvature follows the cursor
    /// (`last_y` re-anchors each step, incremental like a knob drag).
    BpfCurve {
        id: i32,
        segment: usize,
        last_y: f64,
        body_h: f64,
    },
    /// Dragging a multitrack `clip`: the body moves its `offset`, an edge
    /// resizes its `dur`. The cursor maps to a timeline sample through the
    /// lane's `body_x`/`body_w` and the shared `nav_start`/`nav_len`; the
    /// placement follows from a press-time snapshot (`press_sample`,
    /// `orig_offset`, `orig_dur`) so a clamped edge never drifts, snapped to
    /// `grid`.
    Clip {
        id: i32,
        part: interact::ClipPart,
        body_x: f64,
        body_w: f64,
        nav_start: f64,
        nav_len: f64,
        press_sample: f64,
        orig_offset: f64,
        orig_dur: f64,
        grid: f64,
    },
    /// A wire being pulled from a `graph` patch's port: the widget, the port
    /// (member, control) and the widget's area — released over a bus to rewire
    /// it, over empty space to unwire.
    Wire {
        id: i32,
        port: (usize, usize),
        area: Rect,
    },
    /// A break-point of an **automation clip** being dragged in place: the clip
    /// and the point, plus the geometry mapping the cursor back onto the shared
    /// axis and the clip's value range.
    ClipPoint {
        id: i32,
        index: usize,
        rect: Rect,
        body: Rect,
        nav_start: f64,
        nav_len: f64,
        offset: f64,
    },
    /// Dragging a piano-roll note: the body moves it in time and pitch, an edge
    /// resizes its duration. The cursor maps to a region-relative time through
    /// the grid and the shared `nav`, and to a pitch through the visible window
    /// `[lo, hi]`; a press-time snapshot (`press_time`, `orig_*`) keeps a clamped
    /// edge from drifting, snapped to `grid`.
    Note {
        id: i32,
        index: usize,
        part: pianoroll::NotePart,
        grid: Rect,
        nav_start: f64,
        nav_len: f64,
        lo: f32,
        hi: f32,
        press_time: f64,
        orig_start: f64,
        orig_dur: f64,
        snap: f64,
    },
    /// Dragging a note's velocity bar in the velocity lane: the velocity follows
    /// the cursor's height within `lane`.
    Velocity { id: i32, index: usize, lane: Rect },
    /// Dragging an OSC-event marker along the time axis (its `time` follows the
    /// cursor through the grid's shared `nav`, snapped to `grid`).
    OscMark {
        id: i32,
        index: usize,
        grid: Rect,
        nav_start: f64,
        nav_len: f64,
        snap: f64,
    },
    /// A marquee on the piano-roll's empty grid: the time span keeps driving the
    /// **shared time selection** (linked views follow it, exactly as
    /// [`Drag::Select`] does), and the notes inside the time × pitch rectangle
    /// become the widget's multi-note selection.
    SelectNotes {
        id: i32,
        grid: Rect,
        nav_start: f64,
        nav_len: f64,
        lo: f32,
        hi: f32,
        /// The absolute sample under the press.
        anchor: f64,
        /// The (fractional) pitch under the press.
        anchor_pitch: f32,
    },
    /// Dragging a **selected** note moves the whole selection rigidly in time
    /// and pitch. `orig` is the `(index, start, pitch)` snapshot at press time
    /// — the grabbed note's entry leads it (the snap anchor) — so a clamped
    /// block never drifts.
    NoteBlock {
        id: i32,
        grid: Rect,
        nav_start: f64,
        nav_len: f64,
        lo: f32,
        hi: f32,
        press_time: f64,
        press_pitch: f32,
        snap: f64,
        orig: Vec<(usize, f64, f32)>,
    },
    /// Dragging the velocity lane over a **selected** note nudges every selected
    /// velocity by the same delta (relative, from the `(index, velocity)` press
    /// snapshot — each note saturates on its own).
    VelocityBlock {
        id: i32,
        lane: Rect,
        /// The lane velocity under the press (the delta's zero).
        press_velocity: i32,
        orig: Vec<(usize, i32)>,
    },
}

/// One open window: its GPU surface, the per-waveform slots, the painter, the
/// script address its events go to, the pointer/drag state, and the per-`scope`
/// rolling history. The widget tree itself lives in the [`Host`] (single source
/// of truth).
struct WindowState {
    gpu: Gpu,
    waveforms: HashMap<i32, WaveformSlot>,
    /// Per-`spectrogram` GPU resources (one STFT view per channel lane).
    spectrograms: HashMap<i32, SpectrogramSlot>,
    /// Per-`canvas` GPU resources (the compiled user shader + uniforms).
    canvases: HashMap<i32, CanvasView>,
    painter: Painter,
    /// The second mesh pass: editor chrome drawn over the heavy views
    /// (selection, playhead, rulers' overlay parts, cursor readout).
    overlay: Painter,
    origin: SocketAddr,
    cursor: (f64, f64),
    /// Whether Shift is held (Shift+drag pans a timeline view; plain drag
    /// selects).
    shift: bool,
    /// Whether Ctrl is held (Ctrl+click adds/removes a `bpf` breakpoint).
    ctrl: bool,
    /// Whether Alt is held (Alt+click toggles a piano-roll note in/out of the
    /// multi-note selection).
    alt: bool,
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
    /// Live MIDI input: the virtual input port, held open while any open
    /// window has a `midi_in` piano-roll (dropping it closes the port).
    #[cfg(feature = "midi")]
    midi_in: Option<clausters_midi::live::Input>,
    /// Whether the port-open failure was already reported (retrying is cheap,
    /// warning every frame is not).
    #[cfg(feature = "midi")]
    midi_warned: bool,
    /// Held keys being painted: `(window, widget, channel, pitch)` → the index
    /// of the note the matching note-off will close.
    #[cfg(feature = "midi")]
    held: HashMap<(i32, i32, u8, u8), usize>,
    /// Step-entry cursor per `(window, widget)` (timeline samples), used while
    /// the shared playhead is stopped; the last note-off advances it a grid.
    #[cfg(feature = "midi")]
    step: HashMap<(i32, i32), f64>,
    /// The piano-roll note clipboard (Ctrl+C/X/V), normalized to the block's
    /// first onset — host-wide, so notes travel between rolls and windows.
    clipboard: Vec<pianoroll::Note>,
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
            #[cfg(feature = "midi")]
            midi_in: None,
            #[cfg(feature = "midi")]
            midi_warned: false,
            #[cfg(feature = "midi")]
            held: HashMap::new(),
            #[cfg(feature = "midi")]
            step: HashMap::new(),
            clipboard: Vec::new(),
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

    /// Delivers a `bpf` widget's edited breakpoint list — the edit-back
    /// pattern: a **bound** editor forwards the flat `t v shape curve …` list
    /// straight to the audio server (without the `"points"` tag, which names
    /// the event payload, not a server argument); an unbound one emits
    /// `/gui_event id "points" <flat list…>` to the script.
    /// Locates the transport: the timeline position under the cursor becomes the
    /// group's static cursor (drawn at once on every lane, so the click lands
    /// where you see it) and leaves as `/gui_event <id> "locate" <position>` — the
    /// script seeks its playhead there, which is what actually moves the music.
    fn locate_timeline(&mut self, def_id: i32, id: i32, body: Rect, cx: f64) {
        let Some((start, len, _total)) = self.timeline_nav(id) else {
            return;
        };
        let pos = (start + len * ((cx - body.x as f64) / body.w.max(1.0) as f64)).max(0.0);
        let roots = self.host.set_timeline_cursor(id, pos);
        self.emit(
            def_id,
            id,
            vec![OscType::String("locate".into()), OscType::Float(pos as f32)],
        );
        self.redraw_all(&roots);
        self.redraw(def_id);
    }

    /// Whether clip `id` carries a break-point curve (an automation clip).
    fn clip_has_curve(&self, def_id: i32, id: i32) -> bool {
        self.host
            .window_def(def_id)
            .and_then(|t| t.find(id))
            .and_then(track::clip_draw)
            .is_some_and(|clip| !clip.points.is_empty())
    }

    fn emit_points(&self, def_id: i32, widget_id: i32) {
        let Some(args) = self
            .host
            .window_def(def_id)
            .and_then(|t| interact::bpf_event_args(t, widget_id))
        else {
            return;
        };
        if self.host.is_bound(widget_id) {
            self.host.forward_args(widget_id, args[1..].to_vec());
            return;
        }
        self.emit(def_id, widget_id, args);
    }

    /// Delivers a `clip`'s edited placement — the edit-back pattern, the sibling
    /// of [`App::emit_bpf`]: a **bound** clip forwards the flat `offset dur`
    /// straight to the audio server (without the `"clip"` tag); an unbound one
    /// emits `/gui_event id "clip" offset dur` to the script that built it.
    fn emit_clip(&self, def_id: i32, widget_id: i32) {
        let Some(args) = self
            .host
            .window_def(def_id)
            .and_then(|t| interact::clip_event_args(t, widget_id))
        else {
            return;
        };
        if self.host.is_bound(widget_id) {
            self.host.forward_args(widget_id, args[1..].to_vec());
            return;
        }
        self.emit(def_id, widget_id, args);
    }

    /// A piano-roll note's current `(start, dur)` in the host tree.
    fn note_at(&self, def_id: i32, id: i32, index: usize) -> Option<(f64, f64)> {
        match &self.host.window_def(def_id)?.find(id)?.kind {
            WidgetKind::PianoRoll { notes, .. } => notes.get(index).map(|n| (n.start, n.dur)),
            _ => None,
        }
    }

    /// Handles a plain (non-Shift) press on a `pianoroll`: start a note
    /// move/resize (a **selected** note moves the whole selection), a velocity
    /// drag (over a selected note, the whole selection's) or an OSC-marker
    /// drag; Ctrl+click adds or removes a note/marker; Alt+click toggles a note
    /// in/out of the multi-note selection; a press on empty grid drags the
    /// marquee — the shared time selection restricted in pitch, which fills the
    /// selected set. The edit-back gestures, native-only (the browser keeps
    /// display + `/gui_set` parity).
    #[allow(clippy::too_many_arguments)] // one press: a hit, two modifiers, a cursor
    fn pianoroll_press(
        &mut self,
        def_id: i32,
        id: i32,
        h: &interact::PianoRollHit,
        ctrl: bool,
        alt: bool,
        cx: f64,
        cy: f64,
    ) {
        let nav = View {
            start: h.nav.start,
            len: h.nav.len,
        };
        match h.region {
            interact::PrRegion::Grid => {
                // Alt+click toggles a note in/out of the multi-note selection
                // (a non-rectangular selection, one note at a time).
                if alt {
                    if let Some(nh) = h.note {
                        interact::pianoroll_state_edit(&mut self.host, def_id, id, |_, sel| {
                            pianoroll::toggle_selected(sel, nh.index);
                        });
                        self.redraw(def_id);
                    }
                    return;
                }
                if ctrl {
                    match h.note {
                        // Ctrl+click on a note removes it (the selection's
                        // indices shift down past it).
                        Some(nh) => {
                            interact::pianoroll_state_edit(
                                &mut self.host,
                                def_id,
                                id,
                                |notes, sel| {
                                    pianoroll::remove_note(notes, nh.index);
                                    *sel = pianoroll::selection_after_removal(sel, nh.index);
                                },
                            );
                        }
                        // Ctrl+click on empty grid adds a note there, then drags
                        // its end to set the length until release.
                        None => {
                            let time = interact::snap(
                                pianoroll::time_at(h.grid, &nav, 0.0, cx as f32),
                                h.snap,
                            )
                            .max(0.0);
                            let pitch = pianoroll::y_to_pitch(cy as f32, h.lo, h.hi, h.grid)
                                .round()
                                .clamp(h.lo, h.hi);
                            let dur = if h.snap > 0.0 {
                                h.snap
                            } else {
                                (h.nav.len * 0.05).max(1.0)
                            };
                            let index = interact::pianoroll_notes_edit(
                                &mut self.host,
                                def_id,
                                id,
                                |notes| {
                                    pianoroll::insert_note(
                                        notes,
                                        pianoroll::Note::new(time, dur, pitch),
                                    )
                                },
                            );
                            if let Some(index) = index {
                                self.set_drag(
                                    def_id,
                                    Drag::Note {
                                        id,
                                        index,
                                        part: pianoroll::NotePart::End,
                                        grid: h.grid,
                                        nav_start: h.nav.start,
                                        nav_len: h.nav.len,
                                        lo: h.lo,
                                        hi: h.hi,
                                        press_time: time,
                                        orig_start: time,
                                        orig_dur: dur,
                                        snap: h.snap,
                                    },
                                );
                            }
                        }
                    }
                    self.host.sync_track_totals();
                    self.emit_notes(def_id, id);
                    self.redraw(def_id);
                    return;
                }
                match h.note {
                    // Move (body) or resize (edge) the note under the cursor.
                    // Grabbing the body of a **selected** note moves the whole
                    // selection rigidly; grabbing an unselected one drops the
                    // selection first (the single-note gesture, as before).
                    Some(nh) => {
                        let press_time = pianoroll::time_at(h.grid, &nav, 0.0, cx as f32);
                        if nh.part == pianoroll::NotePart::Body {
                            let orig = interact::pianoroll_state_edit(
                                &mut self.host,
                                def_id,
                                id,
                                |notes, sel| {
                                    if !sel.contains(&nh.index) {
                                        sel.clear();
                                        return Vec::new();
                                    }
                                    // The grabbed note's snapshot leads (the
                                    // snap anchor).
                                    let mut idx = sel.clone();
                                    idx.retain(|&i| i != nh.index);
                                    idx.insert(0, nh.index);
                                    idx.iter()
                                        .filter_map(|&i| {
                                            notes.get(i).map(|n| (i, n.start, n.pitch))
                                        })
                                        .collect::<Vec<_>>()
                                },
                            )
                            .unwrap_or_default();
                            if !orig.is_empty() {
                                let press_pitch =
                                    pianoroll::y_to_pitch(cy as f32, h.lo, h.hi, h.grid);
                                self.set_drag(
                                    def_id,
                                    Drag::NoteBlock {
                                        id,
                                        grid: h.grid,
                                        nav_start: h.nav.start,
                                        nav_len: h.nav.len,
                                        lo: h.lo,
                                        hi: h.hi,
                                        press_time,
                                        press_pitch,
                                        snap: h.snap,
                                        orig,
                                    },
                                );
                                return;
                            }
                        }
                        let (orig_start, orig_dur) =
                            self.note_at(def_id, id, nh.index).unwrap_or((0.0, 0.0));
                        self.set_drag(
                            def_id,
                            Drag::Note {
                                id,
                                index: nh.index,
                                part: nh.part,
                                grid: h.grid,
                                nav_start: h.nav.start,
                                nav_len: h.nav.len,
                                lo: h.lo,
                                hi: h.hi,
                                press_time,
                                orig_start,
                                orig_dur,
                                snap: h.snap,
                            },
                        );
                    }
                    // Empty grid: plain drag selects (the heavy-view
                    // convention), and the marquee doubles as the note
                    // selection — the time span restricted in pitch.
                    None => {
                        if let Some((start, len, _)) = self.timeline_nav(id) {
                            let anchor =
                                start + len * ((cx - h.grid.x as f64) / h.grid.w.max(1.0) as f64);
                            self.set_selection(def_id, id, anchor, anchor);
                            let anchor_pitch = pianoroll::y_to_pitch(cy as f32, h.lo, h.hi, h.grid);
                            // The marquee restarts: the previous set drops.
                            interact::pianoroll_state_edit(&mut self.host, def_id, id, |_, sel| {
                                sel.clear()
                            });
                            self.set_drag(
                                def_id,
                                Drag::SelectNotes {
                                    id,
                                    grid: h.grid,
                                    nav_start: start,
                                    nav_len: len,
                                    lo: h.lo,
                                    hi: h.hi,
                                    anchor,
                                    anchor_pitch,
                                },
                            );
                            self.redraw(def_id);
                        }
                    }
                }
            }
            interact::PrRegion::Velocity => {
                if let Some(nh) = h.note {
                    // Over a **selected** note the whole selection's velocities
                    // nudge together (relative, from a press snapshot); over an
                    // unselected one the single bar follows the cursor.
                    let orig =
                        interact::pianoroll_state_edit(&mut self.host, def_id, id, |notes, sel| {
                            if !sel.contains(&nh.index) {
                                return Vec::new();
                            }
                            sel.iter()
                                .filter_map(|&i| notes.get(i).map(|n| (i, n.velocity)))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    if !orig.is_empty() {
                        let lane = h.region_rect;
                        let frac =
                            ((lane.y + lane.h - cy as f32) / lane.h.max(1.0)).clamp(0.0, 1.0);
                        self.set_drag(
                            def_id,
                            Drag::VelocityBlock {
                                id,
                                lane,
                                press_velocity: (frac * 127.0).round() as i32,
                                orig,
                            },
                        );
                        return;
                    }
                    self.set_drag(
                        def_id,
                        Drag::Velocity {
                            id,
                            index: nh.index,
                            lane: h.region_rect,
                        },
                    );
                }
            }
            interact::PrRegion::Osc => {
                if ctrl {
                    match h.osc_index {
                        Some(index) => {
                            interact::pianoroll_osc_edit(&mut self.host, def_id, id, |osc| {
                                if index < osc.len() {
                                    osc.remove(index);
                                }
                            });
                        }
                        None => {
                            let time = interact::snap(
                                pianoroll::time_at(h.grid, &nav, 0.0, cx as f32),
                                h.snap,
                            )
                            .max(0.0);
                            interact::pianoroll_osc_edit(&mut self.host, def_id, id, |osc| {
                                osc.push(pianoroll::OscMark { time, label: None });
                            });
                        }
                    }
                    self.host.sync_track_totals();
                    self.emit_osc(def_id, id);
                    self.redraw(def_id);
                } else if let Some(index) = h.osc_index {
                    self.set_drag(
                        def_id,
                        Drag::OscMark {
                            id,
                            index,
                            grid: h.grid,
                            nav_start: h.nav.start,
                            nav_len: h.nav.len,
                            snap: h.snap,
                        },
                    );
                }
            }
        }
    }

    /// Delivers a piano-roll's edited notes — the edit-back pattern, the sibling
    /// of [`App::emit_clip`]: a **bound** roll forwards the flat note list
    /// straight to the audio server (without the `"notes"` tag); an unbound one
    /// emits `/gui_event id "notes" start dur pitch vel channel …` to the script.
    fn emit_notes(&self, def_id: i32, widget_id: i32) {
        let Some(args) = self
            .host
            .window_def(def_id)
            .and_then(|t| interact::notes_event_args(t, widget_id))
        else {
            return;
        };
        if self.host.is_bound(widget_id) {
            self.host.forward_args(widget_id, args[1..].to_vec());
            return;
        }
        self.emit(def_id, widget_id, args);
    }

    /// Delivers a piano-roll's edited OSC events — `/gui_event id "osc" time label …`.
    fn emit_osc(&self, def_id: i32, widget_id: i32) {
        let Some(args) = self
            .host
            .window_def(def_id)
            .and_then(|t| interact::osc_event_args(t, widget_id))
        else {
            return;
        };
        if self.host.is_bound(widget_id) {
            self.host.forward_args(widget_id, args[1..].to_vec());
            return;
        }
        self.emit(def_id, widget_id, args);
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
        let mut spectrograms = HashMap::new();
        let mut buffer_refs = Vec::new();
        let mut canvases = HashMap::new();
        if let Some(tree) = self.host.window_def(id) {
            collect_timelines(
                tree,
                &gpu,
                &mut waveforms,
                &mut spectrograms,
                &mut buffer_refs,
            );
            collect_canvases(tree, &gpu, &mut canvases);
        }
        // Register each loaded view's data extent with its navigation group
        // (the group timeline spans the longest member).
        for (wid, slot) in &waveforms {
            self.host
                .set_timeline_total(*wid, slot.view.total_samples());
        }
        for (wid, slot) in &spectrograms {
            self.host.set_timeline_total(*wid, slot.total_samples());
        }
        let painter = Painter::new(&gpu.device, gpu.config.format);
        let overlay = Painter::new(&gpu.device, gpu.config.format);

        self.by_winit.insert(winit_id, id);
        self.windows.insert(
            id,
            WindowState {
                gpu,
                waveforms,
                spectrograms,
                canvases,
                painter,
                overlay,
                origin,
                cursor: (0.0, 0.0),
                shift: false,
                ctrl: false,
                alt: false,
                drag: None,
                scopes: HashMap::new(),
                tap_windows: HashMap::new(),
                spectra: HashMap::new(),
            },
        );
        info!("gui_def {id}: opened window \"{title}\"");
        // Plots and clips that name a local file map it now (the bulk path, no
        // OSC); the samples land in the host tree the renderer reads each frame.
        if let Some(root) = self.host.window_def_mut(id) {
            load_plot_paths(root);
            load_clip_bodies(root);
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

    /// Registers timeline widgets (waveform/spectrogram) that reference a
    /// server buffer and queries the audio server for each distinct buffer's
    /// shape (the fetch proceeds on the `/b_info` reply). `refs` is
    /// `(widget_id, bufnum)`.
    fn start_buffer_fetches(&mut self, def_id: i32, refs: Vec<(i32, i32)>) {
        for (widget_id, bufnum) in refs {
            if let Some(query) = self.fetches.want(def_id, widget_id, bufnum) {
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
                samples,
                channels,
                sample_rate,
                wants,
            } => self.finalize_buffer(bufnum, samples, channels, sample_rate, wants),
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
                        rate,
                    ] = group
                    {
                        let step = self.fetches.on_info(
                            *bufnum,
                            (*frames).max(0) as usize,
                            (*channels).max(0) as usize,
                            float_arg(rate),
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

    /// A buffer finished downloading (interleaved, every channel kept): look
    /// up each waiting widget and build its view — a multichannel waveform, or
    /// one STFT lane per channel for a spectrogram. The buffer's `/b_info`
    /// sample rate also fills a widget's unknown `sample_rate`, so its ruler
    /// and readout label real time.
    fn finalize_buffer(
        &mut self,
        bufnum: i32,
        samples: Arc<[f32]>,
        channels: usize,
        sample_rate: f64,
        wants: Vec<WaveWant>,
    ) {
        let channels = channels.max(1);
        info!(
            "buffer {bufnum}: {} frames x {channels} channel(s) loaded into {} view(s)",
            samples.len() / channels,
            wants.len()
        );
        for want in wants {
            let Some(kind) = self
                .host
                .window_def(want.def_id)
                .and_then(|t| t.find(want.widget_id))
                .map(|w| w.kind.clone())
            else {
                continue;
            };
            let Some(ws) = self.windows.get_mut(&want.def_id) else {
                continue;
            };
            match kind {
                WidgetKind::Waveform { base_bucket, .. } => {
                    let data = WaveformData::from_interleaved(&samples, channels, base_bucket);
                    let slot = frame::waveform_slot(data, &ws.gpu);
                    ws.waveforms.insert(want.widget_id, slot);
                }
                WidgetKind::Clip { base_bucket, .. } => {
                    // A clip's take lives in the tree, not on the GPU (its lane
                    // body is flat geometry decimated from the pyramid).
                    let data = Arc::new(WaveformData::from_interleaved(
                        &samples,
                        channels,
                        base_bucket,
                    ));
                    ws.gpu.window.request_redraw();
                    if let Some(w) = self
                        .host
                        .window_def_mut(want.def_id)
                        .and_then(|t| t.find_mut(want.widget_id))
                        && let WidgetKind::Clip { body, .. } = &mut w.kind
                    {
                        *body = Some(data);
                    }
                    continue; // no navigation group, no ruler rate: a lane owns those
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
                    if let Some(slot) = frame::spectrogram_slot(stfts, &ws.gpu) {
                        ws.spectrograms.insert(want.widget_id, slot);
                    }
                }
                _ => continue,
            }
            ws.gpu.window.request_redraw();
            // The fetched buffer's extent joins the widget's navigation group.
            self.host
                .set_timeline_total(want.widget_id, samples.len() / channels);
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
            WidgetKind::Bpf {
                ref points,
                min,
                max,
                duration,
                exp,
                ref label,
                ..
            } => {
                let body = bpf::body(rect, label.is_some());
                let ctrl = self.windows.get(&def_id).is_some_and(|w| w.ctrl);
                let hit_pt = bpf::hit_point(points, body, duration, min, max, exp, cx, cy);
                if ctrl {
                    // Ctrl+click on a point removes it; elsewhere it adds one
                    // at the cursor (which then drags until release).
                    // `None` = nothing changed, `Some(None)` = removed,
                    // `Some(Some(i))` = added at index `i`.
                    let edited: Option<Option<usize>> = match hit_pt {
                        Some(i) => {
                            interact::bpf_edit(&mut self.host, def_id, id, |p, _, _, _, _| {
                                bpf::remove_point(p, i)
                            })
                            .and_then(|removed| removed.then_some(None))
                        }
                        None => interact::bpf_edit(
                            &mut self.host,
                            def_id,
                            id,
                            |p, duration, lo, hi, exp| {
                                bpf::add_point(p, body, duration, lo, hi, exp, cx, cy)
                            },
                        )
                        .map(Some),
                    };
                    if let Some(added) = edited {
                        if let Some(index) = added {
                            self.set_drag(def_id, Drag::BpfPoint { id, index, body });
                        }
                        self.emit_points(def_id, id);
                        self.redraw(def_id);
                    }
                } else if let Some(index) = hit_pt {
                    self.set_drag(def_id, Drag::BpfPoint { id, index, body });
                } else if let Some(segment) = bpf::hit_segment(points, body, duration, cx) {
                    self.set_drag(
                        def_id,
                        Drag::BpfCurve {
                            id,
                            segment,
                            last_y: cy,
                            body_h: body.h.max(1.0) as f64,
                        },
                    );
                }
            }
            WidgetKind::Graph { ref graph, .. } => {
                // A patch's port is the grab point of a rewiring drag; the rest of
                // the patch is display.
                if let Some(port) = graph::port_hit(rect, graph, cx, cy) {
                    self.set_drag(
                        def_id,
                        Drag::Wire {
                            id,
                            port,
                            area: rect,
                        },
                    );
                }
            }
            WidgetKind::Track {
                snap, ref editor, ..
            } => {
                // Shift+drag pans the shared axis (the same gesture the heavy
                // views use), so panning stays available where every plain drag
                // grabs a clip.
                let shift = self.windows.get(&def_id).is_some_and(|w| w.shift);
                if shift {
                    let body = track::lane_body(rect, editor.ruler != Ruler::Off);
                    if let Some((start, _len, _total)) = self.timeline_nav(id) {
                        self.set_drag(
                            def_id,
                            Drag::Pan {
                                id,
                                origin_x: cx,
                                start,
                                body_w: body.w.max(1.0) as f64,
                            },
                        );
                    }
                    return;
                }
                // A press on the lane's **time ruler**, or on empty lane space,
                // *locates* the transport: the multitrack's cursor goes where you
                // point, which is the one gesture a timeline view cannot do
                // without. (Over a clip, the clip's own gestures win.)
                let ruler_on = editor.ruler != Ruler::Off;
                let body = track::lane_body(rect, ruler_on);
                let on_ruler = ruler_on && cy > body.y as f64 + body.h as f64;
                let (fb_w, fb_h) = self.fb(def_id);
                let over_clip =
                    interact::clip_hit(&self.host, def_id, fb_w, fb_h, cx, cy).is_some();
                if on_ruler || (!over_clip && body.contains(cx, cy)) {
                    self.locate_timeline(def_id, id, body, cx);
                    return;
                }
                // A track is the hit target (its clips are placed by the
                // renderer, not the layout engine); find the clip under the
                // cursor and start a move (body) or resize (edge) drag.
                if let Some(h) = interact::clip_hit(&self.host, def_id, fb_w, fb_h, cx, cy) {
                    // An automation clip: a break-point wins over the clip body
                    // (as it wins over a segment in the `bpf` view), and Ctrl+click
                    // adds one - or removes the one under the cursor. The same
                    // gestures, now on a lane.
                    let ctrl = self.windows.get(&def_id).is_some_and(|w| w.ctrl);
                    if h.point.is_some() || (ctrl && self.clip_has_curve(def_id, h.id)) {
                        if ctrl {
                            if interact::clip_point_edit(
                                &mut self.host,
                                def_id,
                                h.id,
                                h.point,
                                h.rect,
                                h.body,
                                &h.nav,
                                h.offset,
                                cx,
                                cy,
                            ) {
                                self.emit_points(def_id, h.id);
                                self.redraw(def_id);
                            }
                        } else if let Some(index) = h.point {
                            self.set_drag(
                                def_id,
                                Drag::ClipPoint {
                                    id: h.id,
                                    index,
                                    rect: h.rect,
                                    body: h.body,
                                    nav_start: h.nav.start,
                                    nav_len: h.nav.len,
                                    offset: h.offset,
                                },
                            );
                        }
                        return;
                    }
                    let press_sample = h.nav.start
                        + h.nav.len * ((cx - h.body.x as f64) / h.body.w.max(1.0) as f64);
                    self.set_drag(
                        def_id,
                        Drag::Clip {
                            id: h.id,
                            part: h.part,
                            body_x: h.body.x as f64,
                            body_w: h.body.w as f64,
                            nav_start: h.nav.start,
                            nav_len: h.nav.len,
                            press_sample,
                            orig_offset: h.offset,
                            orig_dur: h.dur,
                            grid: snap,
                        },
                    );
                }
            }
            WidgetKind::PianoRoll { .. } => {
                let (fb_w, fb_h) = self.fb(def_id);
                let Some(h) = interact::pianoroll_hit(&self.host, def_id, fb_w, fb_h, cx, cy)
                else {
                    return;
                };
                let shift = self.windows.get(&def_id).is_some_and(|w| w.shift);
                let ctrl = self.windows.get(&def_id).is_some_and(|w| w.ctrl);
                let alt = self.windows.get(&def_id).is_some_and(|w| w.alt);
                // A press on the keyboard gutter (left of the grid) pans the pitch
                // window — the keyboard is the piano-roll's vertical axis surface,
                // the counterpart of the heavy views' y-ruler strip.
                if cx < h.grid.x as f64 {
                    let y_start = self
                        .host
                        .window_def(def_id)
                        .and_then(|t| t.find(id))
                        .and_then(|w| w.kind.editor())
                        .map_or(0.0, |e| e.y_view().0);
                    self.set_drag(
                        def_id,
                        Drag::PanY {
                            id,
                            origin_y: cy,
                            y_start,
                            lane_h: h.grid.h.max(1.0) as f64,
                        },
                    );
                    return;
                }
                // Shift+drag pans the shared axis (the heavy-view gesture), so
                // panning stays available where a plain drag edits notes/selects.
                if shift {
                    if let Some((start, _len, _total)) = self.timeline_nav(id) {
                        self.set_drag(
                            def_id,
                            Drag::Pan {
                                id,
                                origin_x: cx,
                                start,
                                body_w: h.grid.w.max(1.0) as f64,
                            },
                        );
                    }
                    return;
                }
                self.pianoroll_press(def_id, id, &h, ctrl, alt, cx, cy);
            }
            WidgetKind::Waveform { ref editor, .. }
            | WidgetKind::Spectrogram { ref editor, .. } => {
                let body = frame::timeline_body(rect, editor);
                // A press on the y-ruler strip left of the body starts a
                // vertical pan of the display window (the strip is the y
                // axis' gesture surface; wheel over it zooms).
                if editor.ruler_y != RulerY::Off && cx < body.x as f64 {
                    let lanes = self.timeline_lanes(def_id, id, &kind);
                    self.set_drag(
                        def_id,
                        Drag::PanY {
                            id,
                            origin_y: cy,
                            y_start: editor.y_view().0,
                            lane_h: (body.h as f64 / lanes.max(1) as f64).max(1.0),
                        },
                    );
                    return;
                }
                let shift = self.windows.get(&def_id).is_some_and(|w| w.shift);
                if let Some((start, len, _)) = self.timeline_nav(id) {
                    if shift {
                        // Shift+drag pans the view (the pre-editor gesture).
                        self.set_drag(
                            def_id,
                            Drag::Pan {
                                id,
                                origin_x: cx,
                                start,
                                body_w: body.w.max(1.0) as f64,
                            },
                        );
                    } else {
                        // Plain drag selects (the editor convention). The press
                        // collapses the selection to the sample under it.
                        let anchor = start + len * ((cx - body.x as f64) / body.w.max(1.0) as f64);
                        self.set_selection(def_id, id, anchor, anchor);
                        self.set_drag(
                            def_id,
                            Drag::Select {
                                id,
                                body_x: body.x as f64,
                                body_w: body.w.max(1.0) as f64,
                                anchor,
                            },
                        );
                        self.redraw(def_id);
                    }
                }
            }
            _ => {}
        }
    }

    /// The navigation window of timeline view `id`'s group:
    /// `(start, len, total)` in timeline samples.
    fn timeline_nav(&self, id: i32) -> Option<(f64, f64, usize)> {
        self.host
            .timeline_nav(id)
            .map(|(nav, total)| (nav.start, nav.len, total))
    }

    /// Repaints every window in `roots` (the windows a group mutation touched).
    fn redraw_all(&self, roots: &[i32]) {
        for root in roots {
            self.redraw(*root);
        }
    }

    /// Writes the selection spanning samples `a..b` (any order, clamped to the
    /// timeline) into view `id`'s navigation group — every member follows —
    /// and emits **one** `"selection" start len` event, carrying the
    /// interacted member's id.
    fn set_selection(&mut self, def_id: i32, id: i32, a: f64, b: f64) {
        let Some((start, len, roots)) = self.host.select_timeline(id, a, b) else {
            return;
        };
        self.redraw_all(&roots);
        self.emit(
            def_id,
            id,
            vec![
                OscType::String("selection".into()),
                OscType::Float(start as f32),
                OscType::Float(len as f32),
            ],
        );
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
                Drag::Pan {
                    id,
                    origin_x,
                    start,
                    body_w,
                } => DragMove::Pan(*id, *origin_x, *start, *body_w),
                Drag::Select {
                    id,
                    body_x,
                    body_w,
                    anchor,
                } => DragMove::Select(*id, *body_x, *body_w, *anchor),
                Drag::PanY {
                    id,
                    origin_y,
                    y_start,
                    lane_h,
                } => DragMove::PanY(*id, *origin_y, *y_start, *lane_h),
                Drag::BpfPoint { id, index, body } => DragMove::BpfPoint(*id, *index, *body),
                Drag::BpfCurve {
                    id,
                    segment,
                    last_y,
                    body_h,
                } => DragMove::BpfCurve(*id, *segment, *last_y, *body_h),
                Drag::Clip {
                    id,
                    part,
                    body_x,
                    body_w,
                    nav_start,
                    nav_len,
                    press_sample,
                    orig_offset,
                    orig_dur,
                    grid,
                } => DragMove::Clip {
                    id: *id,
                    part: *part,
                    body_x: *body_x,
                    body_w: *body_w,
                    nav_start: *nav_start,
                    nav_len: *nav_len,
                    press_sample: *press_sample,
                    orig_offset: *orig_offset,
                    orig_dur: *orig_dur,
                    grid: *grid,
                },
                Drag::Wire { .. } => DragMove::None,
                Drag::ClipPoint {
                    id,
                    index,
                    rect,
                    body,
                    nav_start,
                    nav_len,
                    offset,
                } => DragMove::ClipPoint {
                    id: *id,
                    index: *index,
                    rect: *rect,
                    body: *body,
                    nav_start: *nav_start,
                    nav_len: *nav_len,
                    offset: *offset,
                },
                Drag::Note {
                    id,
                    index,
                    part,
                    grid,
                    nav_start,
                    nav_len,
                    lo,
                    hi,
                    press_time,
                    orig_start,
                    orig_dur,
                    snap,
                } => DragMove::Note {
                    id: *id,
                    index: *index,
                    part: *part,
                    grid: *grid,
                    nav_start: *nav_start,
                    nav_len: *nav_len,
                    lo: *lo,
                    hi: *hi,
                    press_time: *press_time,
                    orig_start: *orig_start,
                    orig_dur: *orig_dur,
                    snap: *snap,
                },
                Drag::Velocity { id, index, lane } => DragMove::Velocity(*id, *index, *lane),
                Drag::OscMark {
                    id,
                    index,
                    grid,
                    nav_start,
                    nav_len,
                    snap,
                } => DragMove::OscMark {
                    id: *id,
                    index: *index,
                    grid: *grid,
                    nav_start: *nav_start,
                    nav_len: *nav_len,
                    snap: *snap,
                },
                Drag::SelectNotes {
                    id,
                    grid,
                    nav_start,
                    nav_len,
                    lo,
                    hi,
                    anchor,
                    anchor_pitch,
                } => DragMove::SelectNotes {
                    id: *id,
                    grid: *grid,
                    nav_start: *nav_start,
                    nav_len: *nav_len,
                    lo: *lo,
                    hi: *hi,
                    anchor: *anchor,
                    anchor_pitch: *anchor_pitch,
                },
                Drag::NoteBlock {
                    id,
                    grid,
                    nav_start,
                    nav_len,
                    lo,
                    hi,
                    press_time,
                    press_pitch,
                    snap,
                    orig,
                } => DragMove::NoteBlock {
                    id: *id,
                    grid: *grid,
                    nav_start: *nav_start,
                    nav_len: *nav_len,
                    lo: *lo,
                    hi: *hi,
                    press_time: *press_time,
                    press_pitch: *press_pitch,
                    snap: *snap,
                    orig: orig.clone(),
                },
                Drag::VelocityBlock {
                    id,
                    lane,
                    press_velocity,
                    orig,
                } => DragMove::VelocityBlock {
                    id: *id,
                    lane: *lane,
                    press_velocity: *press_velocity,
                    orig: orig.clone(),
                },
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
            Some(DragMove::Pan(id, origin_x, start, body_w)) => {
                self.pan_timeline(def_id, id, start, (cx - origin_x) / body_w);
            }
            Some(DragMove::PanY(id, origin_y, y_start, lane_h)) => {
                // Dragging down moves the window down with the cursor;
                // absolute from the snapshot, so a clamped edge never drifts.
                let y_len = self
                    .host
                    .window_def(def_id)
                    .and_then(|t| t.find(id))
                    .and_then(|w| w.kind.editor())
                    .map_or(1.0, |e| e.y_view().1);
                let start = y_start + (cy - origin_y) / lane_h * y_len;
                self.set_y_view(def_id, id, start, y_len);
            }
            Some(DragMove::BpfPoint(id, index, body)) => {
                interact::bpf_edit(&mut self.host, def_id, id, |p, duration, lo, hi, exp| {
                    bpf::move_point(p, index, body, duration, lo, hi, exp, cx, cy);
                });
                self.emit_points(def_id, id);
                self.redraw(def_id);
            }
            Some(DragMove::BpfCurve(id, segment, last_y, body_h)) => {
                // Incremental like a knob: the upward step bends the curve so
                // the segment's middle follows the cursor.
                let dy_frac = (last_y - cy) / body_h;
                interact::bpf_edit(&mut self.host, def_id, id, |p, _, _, _, _| {
                    bpf::drag_curve(p, segment, dy_frac);
                });
                if let Some(Drag::BpfCurve { last_y, .. }) =
                    self.windows.get_mut(&def_id).and_then(|w| w.drag.as_mut())
                {
                    *last_y = cy;
                }
                self.emit_points(def_id, id);
                self.redraw(def_id);
            }
            Some(DragMove::Select(id, body_x, body_w, anchor)) => {
                let (start, len) = match self.timeline_nav(id) {
                    Some((start, len, _)) => (start, len),
                    None => return,
                };
                let cur = start + len * ((cx - body_x) / body_w);
                self.set_selection(def_id, id, anchor, cur);
            }
            Some(DragMove::Clip {
                id,
                part,
                body_x,
                body_w,
                nav_start,
                nav_len,
                press_sample,
                orig_offset,
                orig_dur,
                grid,
            }) => {
                // Map the cursor to a timeline sample and shift by the press-time
                // grab so a clamped edge never drifts; snap to the track grid.
                let sample = nav_start + nav_len * ((cx - body_x) / body_w.max(1.0));
                let delta = sample - press_sample;
                let end = orig_offset + orig_dur;
                let (new_offset, new_dur) = match part {
                    interact::ClipPart::Body => {
                        (interact::snap(orig_offset + delta, grid), orig_dur)
                    }
                    interact::ClipPart::End => {
                        let new_end = interact::snap(end + delta, grid).max(orig_offset);
                        (orig_offset, new_end - orig_offset)
                    }
                    interact::ClipPart::Start => {
                        let new_off = interact::snap(orig_offset + delta, grid).clamp(0.0, end);
                        (new_off, end - new_off)
                    }
                };
                interact::clip_set(&mut self.host, def_id, id, Some(new_offset), Some(new_dur));
                // The lane's extent moved with the clip: re-register it, so the
                // shared axis grows when a clip is dragged past the end.
                self.host.sync_track_totals();
                self.emit_clip(def_id, id);
                self.redraw(def_id);
            }
            Some(DragMove::ClipPoint {
                id,
                index,
                rect,
                body,
                nav_start,
                nav_len,
                offset,
            }) => {
                // The curve of an automation clip, edited in place: the cursor maps
                // back through the shared axis (time) and the clip's value range,
                // then the point moves with the `bpf` model's own semantics.
                let nav = View {
                    start: nav_start,
                    len: nav_len,
                };
                if interact::clip_point_move(
                    &mut self.host,
                    def_id,
                    id,
                    index,
                    rect,
                    body,
                    &nav,
                    offset,
                    cx,
                    cy,
                ) {
                    self.emit_points(def_id, id);
                    self.redraw(def_id);
                }
            }
            Some(DragMove::Note {
                id,
                index,
                part,
                grid,
                nav_start,
                nav_len,
                lo,
                hi,
                press_time,
                orig_start,
                orig_dur,
                snap,
            }) => {
                // Map the cursor to a region-relative time and (for a body move)
                // a pitch; a press-time snapshot keeps a clamped edge from
                // drifting, snapped to the note grid.
                let time = nav_start + nav_len * ((cx - grid.x as f64) / grid.w.max(1.0) as f64);
                interact::pianoroll_notes_edit(&mut self.host, def_id, id, |notes| match part {
                    pianoroll::NotePart::Body => {
                        let delta = time - press_time;
                        let new_start = interact::snap(orig_start + delta, snap);
                        let pitch = pianoroll::y_to_pitch(cy as f32, lo, hi, grid);
                        pianoroll::move_note(notes, index, new_start, pitch, lo, hi);
                        // The duration is preserved by move_note; re-assert it in
                        // case a prior edit changed it under a running drag.
                        if let Some(n) = notes.get_mut(index) {
                            n.dur = orig_dur;
                        }
                    }
                    other => {
                        pianoroll::resize_note(
                            notes,
                            index,
                            other,
                            interact::snap(time, snap),
                            1.0,
                        );
                    }
                });
                self.host.sync_track_totals();
                self.emit_notes(def_id, id);
                self.redraw(def_id);
            }
            Some(DragMove::Velocity(id, index, lane)) => {
                let frac = ((lane.y + lane.h - cy as f32) / lane.h.max(1.0)).clamp(0.0, 1.0);
                let vel = (frac * 127.0).round() as i32;
                interact::pianoroll_notes_edit(&mut self.host, def_id, id, |notes| {
                    pianoroll::set_velocity(notes, index, vel);
                });
                self.emit_notes(def_id, id);
                self.redraw(def_id);
            }
            Some(DragMove::OscMark {
                id,
                index,
                grid,
                nav_start,
                nav_len,
                snap,
            }) => {
                let time = nav_start + nav_len * ((cx - grid.x as f64) / grid.w.max(1.0) as f64);
                interact::pianoroll_osc_edit(&mut self.host, def_id, id, |osc| {
                    if let Some(m) = osc.get_mut(index) {
                        m.time = interact::snap(time, snap).max(0.0);
                    }
                });
                self.host.sync_track_totals();
                self.emit_osc(def_id, id);
                self.redraw(def_id);
            }
            Some(DragMove::SelectNotes {
                id,
                grid,
                nav_start,
                nav_len,
                lo,
                hi,
                anchor,
                anchor_pitch,
            }) => {
                // The marquee: the time span keeps driving the shared selection
                // (linked views follow it), and the time × pitch rectangle
                // fills the widget's multi-note selection.
                let cur = nav_start + nav_len * ((cx - grid.x as f64) / grid.w.max(1.0) as f64);
                self.set_selection(def_id, id, anchor, cur);
                let pitch = pianoroll::y_to_pitch(cy as f32, lo, hi, grid);
                interact::pianoroll_state_edit(&mut self.host, def_id, id, |notes, sel| {
                    *sel = pianoroll::notes_in_rect(notes, anchor, cur, anchor_pitch, pitch);
                });
                self.redraw(def_id);
            }
            Some(DragMove::NoteBlock {
                id,
                grid,
                nav_start,
                nav_len,
                lo,
                hi,
                press_time,
                press_pitch,
                snap,
                orig,
            }) => {
                // The block move: the grabbed note (the leading snapshot entry)
                // snaps to the note grid, and the whole selection moves rigidly
                // by that delta — the core clamps it as one.
                let time = nav_start + nav_len * ((cx - grid.x as f64) / grid.w.max(1.0) as f64);
                let dt = match orig.first() {
                    Some((_, s0, _)) => interact::snap(s0 + (time - press_time), snap) - s0,
                    None => 0.0,
                };
                let dp = pianoroll::y_to_pitch(cy as f32, lo, hi, grid) - press_pitch;
                interact::pianoroll_notes_edit(&mut self.host, def_id, id, |notes| {
                    pianoroll::move_notes_from(notes, &orig, dt, dp, lo, hi);
                });
                self.host.sync_track_totals();
                self.emit_notes(def_id, id);
                self.redraw(def_id);
            }
            Some(DragMove::VelocityBlock {
                id,
                lane,
                press_velocity,
                orig,
            }) => {
                let frac = ((lane.y + lane.h - cy as f32) / lane.h.max(1.0)).clamp(0.0, 1.0);
                let dv = (frac * 127.0).round() as i32 - press_velocity;
                interact::pianoroll_notes_edit(&mut self.host, def_id, id, |notes| {
                    pianoroll::nudge_velocities_from(notes, &orig, dv);
                });
                self.emit_notes(def_id, id);
                self.redraw(def_id);
            }
            Some(DragMove::None) | None => {}
        }
    }

    fn pan_timeline(&mut self, def_id: i32, id: i32, start: f64, dx_fraction: f64) {
        let Some((_, len, _)) = self.timeline_nav(id) else {
            return;
        };
        let roots = self.host.pan_timeline(id, start - dx_fraction * len);
        self.emit_view(def_id, id);
        self.redraw_all(&roots);
    }

    /// Emits a timeline view's visible range as a `/gui_event id "view" start len`
    /// — once per gesture step, carrying the interacted member's id (linked
    /// members repaint but do not re-emit).
    fn emit_view(&self, def_id: i32, id: i32) {
        if let Some((start, len, _)) = self.timeline_nav(id) {
            self.emit(
                def_id,
                id,
                vec![
                    OscType::String("view".into()),
                    OscType::Float(start as f32),
                    OscType::Float(len as f32),
                ],
            );
        }
    }

    /// The lane count timeline view `id` stacks on screen (overlaid waveform
    /// traces share one lane) — the divisor for lane-relative y gestures.
    fn timeline_lanes(&self, def_id: i32, id: i32, kind: &WidgetKind) -> usize {
        let Some(ws) = self.windows.get(&def_id) else {
            return 1;
        };
        match kind {
            WidgetKind::Waveform { overlay: true, .. } => 1,
            WidgetKind::Waveform { .. } => ws
                .waveforms
                .get(&id)
                .map_or(1, |s| s.view.num_channels().max(1)),
            WidgetKind::Spectrogram { .. } => {
                ws.spectrograms.get(&id).map_or(1, |s| s.views.len().max(1))
            }
            _ => 1,
        }
    }

    /// Writes timeline view `id`'s vertical display window (clamped) into its
    /// editor props and emits the `"view_y" y_start y_len` event — the
    /// vertical sibling of [`Self::emit_view`]'s range.
    fn set_y_view(&mut self, def_id: i32, id: i32, start: f64, len: f64) {
        let (start, len) = crate::viewport::clamp_span(start, len);
        if let Some(editor) = self
            .host
            .window_def_mut(def_id)
            .and_then(|t| t.find_mut(id))
            .and_then(|w| w.kind.editor_mut())
        {
            (editor.y_start, editor.y_len) = (start, len);
        }
        self.emit(
            def_id,
            id,
            vec![
                OscType::String("view_y".into()),
                OscType::Float(start as f32),
                OscType::Float(len as f32),
            ],
        );
        self.redraw(def_id);
    }

    /// Anchor-preserving vertical zoom of timeline view `id`: `anchor` in
    /// display coordinates (0 = lane bottom, 1 = lane top).
    fn zoom_timeline_y(&mut self, def_id: i32, id: i32, factor: f64, anchor: f64) {
        let Some((y0, ylen)) = self
            .host
            .window_def(def_id)
            .and_then(|t| t.find(id))
            .and_then(|w| w.kind.editor())
            .map(|e| e.y_view())
        else {
            return;
        };
        let (start, len) = crate::viewport::zoom_span(y0, ylen, factor, anchor);
        self.set_y_view(def_id, id, start, len);
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
            Some(Drag::Wire { id, port, area }) => {
                // Released over a bus: the control is rewired to it. Over empty
                // space: unwired (the bus is reported empty). Either way the tree
                // is written and the edit leaves as a flat `"wire"` event, so the
                // script updates the logical group and re-realizes it.
                let (cx, cy) = self.windows.get(&def_id).map_or((0.0, 0.0), |w| w.cursor);
                if let Some((member, control, bus)) =
                    interact::wire_set(&mut self.host, def_id, id, port, area, cx, cy)
                {
                    self.emit(
                        def_id,
                        id,
                        vec![
                            OscType::String("wire".into()),
                            OscType::Int(member as i32),
                            OscType::String(control),
                            OscType::String(bus),
                        ],
                    );
                    self.redraw(def_id);
                }
            }
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
        let cursor = self.windows.get(&def_id).map(|w| w.cursor);
        let inputs = frame::FrameInputs {
            bus: self.shm.as_deref(),
            node_trees: &self.node_trees,
            active_button,
            server_attached,
            sample_rate: self.shm.as_ref().map_or(0.0, |s| s.sample_rate()),
            sample_clock: self.shm.as_ref().map_or(0.0, |s| s.sample_clock()),
            cursor,
            timelines: self.host.timelines(),
            // A rewiring drag in flight draws its wire to the pointer.
            wiring: match self.windows.get(&def_id).and_then(|w| w.drag.as_ref()) {
                Some(Drag::Wire { id, port, .. }) => {
                    cursor.map(|(cx, cy)| (*id, *port, (cx as f32, cy as f32)))
                }
                _ => None,
            },
        };
        let Some(ws) = self.windows.get_mut(&def_id) else {
            return;
        };
        frame::render(
            &mut ws.gpu,
            &mut ws.painter,
            &mut ws.overlay,
            &mut ws.waveforms,
            &mut ws.spectrograms,
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
    Pan(i32, f64, f64, f64),
    Select(i32, f64, f64, f64),
    PanY(i32, f64, f64, f64),
    BpfPoint(i32, usize, Rect),
    BpfCurve(i32, usize, f64, f64),
    Clip {
        id: i32,
        part: interact::ClipPart,
        body_x: f64,
        body_w: f64,
        nav_start: f64,
        nav_len: f64,
        press_sample: f64,
        orig_offset: f64,
        orig_dur: f64,
        grid: f64,
    },
    ClipPoint {
        id: i32,
        index: usize,
        rect: Rect,
        body: Rect,
        nav_start: f64,
        nav_len: f64,
        offset: f64,
    },
    Note {
        id: i32,
        index: usize,
        part: pianoroll::NotePart,
        grid: Rect,
        nav_start: f64,
        nav_len: f64,
        lo: f32,
        hi: f32,
        press_time: f64,
        orig_start: f64,
        orig_dur: f64,
        snap: f64,
    },
    Velocity(i32, usize, Rect),
    OscMark {
        id: i32,
        index: usize,
        grid: Rect,
        nav_start: f64,
        nav_len: f64,
        snap: f64,
    },
    SelectNotes {
        id: i32,
        grid: Rect,
        nav_start: f64,
        nav_len: f64,
        lo: f32,
        hi: f32,
        anchor: f64,
        anchor_pitch: f32,
    },
    NoteBlock {
        id: i32,
        grid: Rect,
        nav_start: f64,
        nav_len: f64,
        lo: f32,
        hi: f32,
        press_time: f64,
        press_pitch: f32,
        snap: f64,
        orig: Vec<(usize, f64, f32)>,
    },
    VelocityBlock {
        id: i32,
        lane: Rect,
        press_velocity: i32,
        orig: Vec<(usize, i32)>,
    },
    None,
}

/// Walks the tree building the timeline views (waveform and spectrogram). A
/// `cache`/`path` resource is loaded **now** from a mapped local file (the
/// bulk path, no OSC); a server-`buffer` reference with no data is deferred as
/// a `(widget_id, bufnum)` entry in `buffer_refs` for the client leg to fetch;
/// inline/blob (and empty) samples build a slot directly.
fn collect_timelines(
    widget: &Widget,
    gpu: &Gpu,
    waveforms: &mut HashMap<i32, WaveformSlot>,
    spectrograms: &mut HashMap<i32, SpectrogramSlot>,
    buffer_refs: &mut Vec<(i32, i32)>,
) {
    match (&widget.kind, widget.id) {
        (
            WidgetKind::Waveform {
                samples,
                base_bucket,
                buffer,
                path,
                cache,
                channels,
                ..
            },
            Some(id),
        ) => {
            if cache.is_some() || path.is_some() {
                // Bulk path: map a local resource (raw samples or a prebuilt
                // cache) through the BulkLoader seam, then build the GPU slot.
                if let Some(data) =
                    MmapLoader.waveform(cache.as_deref(), path.as_deref(), *channels, *base_bucket)
                {
                    waveforms.insert(id, frame::waveform_slot(data, gpu));
                }
            } else if let (Some(bufnum), true) = (buffer, samples.is_empty()) {
                // A server buffer with no inline data: fetch it over the leg.
                buffer_refs.push((id, *bufnum));
            } else {
                waveforms.insert(
                    id,
                    frame::waveform_slot(
                        WaveformData::from_interleaved(samples, *channels, *base_bucket),
                        gpu,
                    ),
                );
            }
        }
        (
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
            },
            Some(id),
        ) => {
            if let Some(cache) = cache {
                // A prebuilt (single-channel) STFT cache, parsed directly.
                if let Some(stft) = MmapLoader
                    .file_bytes(cache)
                    .and_then(|bytes| Stft::from_bytes(&bytes))
                {
                    if let Some(slot) = frame::spectrogram_slot(vec![stft], gpu) {
                        spectrograms.insert(id, slot);
                    }
                } else {
                    warn!(
                        "spectrogram {id}: cannot parse STFT cache {}",
                        cache.display()
                    );
                }
            } else if let Some(path) = path {
                if let Some(split) = MmapLoader.raw_channels(path, *channels) {
                    let stfts = frame::stft_lanes(split, *window_size, *hop, *sample_rate);
                    if let Some(slot) = frame::spectrogram_slot(stfts, gpu) {
                        spectrograms.insert(id, slot);
                    }
                }
            } else if let (Some(bufnum), true) = (buffer, samples.is_empty()) {
                buffer_refs.push((id, *bufnum));
            } else if !samples.is_empty() {
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
        }
        (
            WidgetKind::Clip {
                samples, buffer, ..
            },
            Some(id),
        ) => {
            // A clip naming a server buffer with no inline body: fetch it over
            // the leg, exactly like a waveform (the `cache`/`path` bulk bodies
            // are mapped in `load_clip_bodies` when the window opens).
            if let (Some(bufnum), true) = (buffer, samples.is_empty()) {
                buffer_refs.push((id, *bufnum));
            }
        }
        _ => {}
    }
    for child in &widget.children {
        collect_timelines(child, gpu, waveforms, spectrograms, buffer_refs);
    }
}

/// A numeric OSC argument as `f64` (0.0 when it is neither float nor int).
fn float_arg(arg: &OscType) -> f64 {
    match arg {
        OscType::Float(x) => *x as f64,
        OscType::Double(x) => *x,
        OscType::Int(n) => *n as f64,
        OscType::Long(n) => *n as f64,
        _ => 0.0,
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

/// Maps the local resource (`cache` or `path`) of every clip that names one,
/// through the same [`BulkLoader`](super::BulkLoader) seam the waveform view
/// uses — so a minutes-long take reaches a lane as a peak pyramid, never as JSON
/// over OSC. The loaded body lands in the host tree (like a plot's samples), no
/// GPU slot: a lane draws flat geometry decimated from the pyramid.
#[allow(clippy::type_complexity)]
fn load_clip_bodies(widget: &mut Widget) {
    if let WidgetKind::Clip {
        body,
        path,
        cache,
        channels,
        base_bucket,
        ..
    } = &mut widget.kind
        && body.is_none()
        && (path.is_some() || cache.is_some())
        && let Some(data) =
            MmapLoader.waveform(cache.as_deref(), path.as_deref(), *channels, *base_bucket)
    {
        *body = Some(Arc::new(data));
    }
    for child in &mut widget.children {
        load_clip_bodies(child);
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

        // Live MIDI input painting notes: while any open window has a
        // `midi_in` roll, the virtual input port is held open and drained at
        // the frame cadence (dropping it when the last such roll goes closes
        // the port).
        #[cfg(feature = "midi")]
        {
            let rolls = self.midi_rolls();
            if rolls.is_empty() {
                self.midi_in = None;
            } else {
                if self.midi_in.is_none() {
                    self.midi_in = clausters_midi::live::Input::open("clausters-gui");
                    if self.midi_in.is_none() && !self.midi_warned {
                        tracing::warn!("could not open the virtual MIDI input port");
                        self.midi_warned = true;
                    }
                }
                self.drain_midi(&rolls);
                let t = now + FRAME;
                next_wake = Some(next_wake.map_or(t, |w| w.min(t)));
            }
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
            WindowEvent::ModifiersChanged(mods) => {
                if let Some(ws) = self.windows.get_mut(&def_id) {
                    ws.shift = mods.state().shift_key();
                    ws.ctrl = mods.state().control_key();
                    ws.alt = mods.state().alt_key();
                }
            }
            WindowEvent::CursorLeft { .. } => {
                if let Some(ws) = self.windows.get_mut(&def_id) {
                    // Off-window: the cursor readout hides (nothing contains it).
                    ws.cursor = (-1.0, -1.0);
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
                if let Some((id, rect, kind)) = self.hit(def_id, cx, cy)
                    && let Some(editor) = kind.editor()
                {
                    let factor = 0.85f64.powf(steps);
                    // The piano-roll's vertical axis is the keyboard gutter, not a
                    // y-ruler strip: wheel over it zooms the pitch window, wheel
                    // over the grid zooms the shared time axis.
                    if let WidgetKind::PianoRoll {
                        osc_lane,
                        velocity_lane,
                        ..
                    } = &kind
                    {
                        let r = pianoroll::regions(
                            rect,
                            editor.ruler != Ruler::Off,
                            *osc_lane,
                            *velocity_lane,
                        );
                        if cx < r.grid.x as f64 {
                            let rel =
                                ((cy - r.grid.y as f64) / r.grid.h.max(1.0) as f64).clamp(0.0, 1.0);
                            self.zoom_timeline_y(def_id, id, factor, 1.0 - rel);
                        } else {
                            self.zoom_timeline(def_id, id, r.grid, cx, factor);
                        }
                        return;
                    }
                    // A lane's body is the strip right of its header (and above
                    // its ruler); a heavy view's is its rect minus its rulers.
                    let body = match kind {
                        WidgetKind::Track { .. } => {
                            track::lane_body(rect, editor.ruler != Ruler::Off)
                        }
                        _ => frame::timeline_body(rect, editor),
                    };
                    if editor.ruler_y != RulerY::Off && cx < body.x as f64 {
                        // Wheel over the y-ruler strip zooms the vertical
                        // display window, anchored at the cursor's height
                        // within the lane under it.
                        let lanes = self.timeline_lanes(def_id, id, &kind);
                        let lane = frame::lane_rect(body, lanes, frame::lane_at(body, lanes, cy));
                        let rel = ((cy - lane.y as f64) / lane.h.max(1.0) as f64).clamp(0.0, 1.0);
                        self.zoom_timeline_y(def_id, id, factor, 1.0 - rel);
                    } else {
                        self.zoom_timeline(def_id, id, body, cx, factor);
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let ctrl = self.windows.get(&def_id).is_some_and(|w| w.ctrl);
                match event.logical_key {
                    Key::Named(NamedKey::Escape) => self.user_close(def_id, event_loop),
                    Key::Named(NamedKey::Delete) | Key::Named(NamedKey::Backspace) => {
                        self.delete_selected_notes(def_id)
                    }
                    Key::Character(ref c) if ctrl && c.eq_ignore_ascii_case("c") => {
                        self.copy_selected_notes(def_id, false)
                    }
                    Key::Character(ref c) if ctrl && c.eq_ignore_ascii_case("x") => {
                        self.copy_selected_notes(def_id, true)
                    }
                    Key::Character(ref c) if ctrl && c.eq_ignore_ascii_case("v") => {
                        self.paste_notes_at_cursor(def_id)
                    }
                    Key::Character(ref c) if c.eq_ignore_ascii_case("q") => {
                        self.quantize_roll(def_id)
                    }
                    Key::Character(ref c) if c.eq_ignore_ascii_case("r") => {
                        self.reset_timelines(def_id)
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
    fn zoom_timeline(&mut self, def_id: i32, id: i32, body: Rect, cx: f64, factor: f64) {
        let anchor = ((cx - body.x as f64) / body.w.max(1.0) as f64).clamp(0.0, 1.0);
        let roots = self.host.zoom_timeline(id, factor, anchor);
        self.emit_view(def_id, id);
        self.redraw_all(&roots);
    }

    /// Every `midi_in` piano-roll in an open window, as `(window, widget)`.
    #[cfg(feature = "midi")]
    fn midi_rolls(&self) -> Vec<(i32, i32)> {
        fn collect(w: &Widget, def_id: i32, out: &mut Vec<(i32, i32)>) {
            if let (WidgetKind::PianoRoll { midi_in: true, .. }, Some(id)) = (&w.kind, w.id) {
                out.push((def_id, id));
            }
            for c in &w.children {
                collect(c, def_id, out);
            }
        }
        let mut out = Vec::new();
        for &def_id in self.windows.keys() {
            if let Some(tree) = self.host.window_def(def_id) {
                collect(tree, def_id, &mut out);
            }
        }
        out
    }

    /// Drain the virtual input port and paint each note event into every
    /// `midi_in` roll. A note-on inserts a **held** note — at the running
    /// playhead (live recording), or at the step cursor when the transport is
    /// stopped (step entry) — and the matching note-off closes it: the real
    /// held duration in playhead mode, or a grid step (advancing the cursor
    /// once all keys are up) in step mode.
    #[cfg(feature = "midi")]
    fn drain_midi(&mut self, rolls: &[(i32, i32)]) {
        let mut events = Vec::new();
        if let Some(input) = &self.midi_in {
            while let Some(msg) = input.poll() {
                if let Some(ev) = clausters_midi::parse_note(&msg) {
                    events.push(ev);
                }
            }
        }
        if events.is_empty() {
            return;
        }
        for &(def_id, id) in rolls {
            for &ev in &events {
                self.paint_note(def_id, id, ev);
            }
            self.host.sync_track_totals();
            self.emit_notes(def_id, id);
            self.redraw(def_id);
        }
    }

    /// Paint one live note event into a roll (see [`App::drain_midi`]).
    #[cfg(feature = "midi")]
    fn paint_note(&mut self, def_id: i32, id: i32, ev: clausters_midi::NoteEvent) {
        use clausters_midi::NoteEvent;
        let playhead = self.playhead_sample(def_id, id);
        let snap = self.roll_snap(def_id, id);
        // The painted length: the note grid, else a visible sliver of the view
        // (the Ctrl+click default) — note-off then sets the real duration.
        let dur = if snap > 0.0 {
            snap
        } else {
            self.timeline_nav(id)
                .map_or(1.0, |(_, len, _)| (len * 0.05).max(1.0))
        };
        match ev {
            NoteEvent::On {
                channel,
                pitch,
                velocity,
            } => {
                let pos = match playhead {
                    Some(p) => interact::snap(p, snap).max(0.0),
                    None => *self.step.entry((def_id, id)).or_insert(0.0),
                };
                let index = interact::pianoroll_notes_edit(&mut self.host, def_id, id, |notes| {
                    pianoroll::insert_note(
                        notes,
                        pianoroll::Note {
                            start: pos,
                            dur,
                            pitch: pitch as f32,
                            velocity: velocity as i32,
                            channel: channel as i32,
                        },
                    )
                });
                if let Some(index) = index {
                    self.held.insert((def_id, id, channel, pitch), index);
                }
            }
            NoteEvent::Off { channel, pitch } => {
                let Some(index) = self.held.remove(&(def_id, id, channel, pitch)) else {
                    return;
                };
                if let Some(now) = playhead {
                    // Live recording: the key was held this long.
                    interact::pianoroll_notes_edit(&mut self.host, def_id, id, |notes| {
                        if let Some(n) = notes.get_mut(index) {
                            n.dur = (now - n.start).max(1.0);
                        }
                    });
                } else if !self
                    .held
                    .keys()
                    .any(|(d, w, _, _)| (*d, *w) == (def_id, id))
                {
                    // Step entry: the last key up advances the cursor a grid
                    // (a chord steps once).
                    *self.step.entry((def_id, id)).or_insert(0.0) += dur;
                }
            }
        }
    }

    /// The shared playhead's current sample for a widget while it is running
    /// (`playhead_at` anchored to the engine clock), else `None`.
    #[cfg(feature = "midi")]
    fn playhead_sample(&self, def_id: i32, id: i32) -> Option<f64> {
        let tree = self.host.window_def(def_id)?;
        let e = tree.find(id)?.kind.editor()?;
        let clock = self.shm.as_ref().map_or(0.0, |s| s.sample_clock());
        (e.playhead_at >= 0.0 && clock > 0.0).then_some(clock - e.playhead_at)
    }

    /// A piano-roll's `snap` grid (0 when none or not a roll).
    #[cfg(feature = "midi")]
    fn roll_snap(&self, def_id: i32, id: i32) -> f64 {
        match self
            .host
            .window_def(def_id)
            .and_then(|t| t.find(id))
            .map(|w| &w.kind)
        {
            Some(WidgetKind::PianoRoll { snap, .. }) => *snap,
            _ => 0.0,
        }
    }

    /// `q` over a piano-roll: quantize the selected notes' onsets (all of them
    /// when nothing is selected) to the widget's `snap` grid — the same grid a
    /// drag snaps to. Durations are kept; a roll with no grid is left alone.
    /// (The client-side counterpart, in beats over the model, is the Python
    /// `Timeline.quantize` — the standalone host cannot reach it, hence both.)
    fn quantize_roll(&mut self, def_id: i32) {
        let Some((cx, cy)) = self.windows.get(&def_id).map(|w| w.cursor) else {
            return;
        };
        let Some((id, _rect, WidgetKind::PianoRoll { snap, .. })) = self.hit(def_id, cx, cy) else {
            return;
        };
        let moved = interact::pianoroll_state_edit(&mut self.host, def_id, id, |notes, sel| {
            pianoroll::quantize_notes(notes, sel, snap)
        })
        .unwrap_or(false);
        if moved {
            self.host.sync_track_totals();
            self.emit_notes(def_id, id);
            self.redraw(def_id);
        }
    }

    /// Ctrl+C / Ctrl+X over a piano-roll: copy the selected notes to the host
    /// clipboard, normalized to the block's first onset (a cut also removes
    /// them). The clipboard is host-wide, so a block travels between rolls and
    /// windows. A no-op when the cursor is elsewhere or nothing is selected.
    fn copy_selected_notes(&mut self, def_id: i32, cut: bool) {
        let Some((cx, cy)) = self.windows.get(&def_id).map(|w| w.cursor) else {
            return;
        };
        let Some((id, _rect, WidgetKind::PianoRoll { .. })) = self.hit(def_id, cx, cy) else {
            return;
        };
        let copied = interact::pianoroll_state_edit(&mut self.host, def_id, id, |notes, sel| {
            let clip = pianoroll::copy_notes(notes, sel);
            if cut && !clip.is_empty() {
                pianoroll::remove_notes(notes, sel);
                sel.clear();
            }
            clip
        })
        .unwrap_or_default();
        if copied.is_empty() {
            return;
        }
        self.clipboard = copied;
        if cut {
            self.host.sync_track_totals();
            self.emit_notes(def_id, id);
            self.redraw(def_id);
        }
    }

    /// Ctrl+V over a piano-roll: paste the clipboard with its first onset at
    /// the cursor's time (snapped to the note grid), original pitches and
    /// spread kept. The pasted block becomes the new selection, ready to drag
    /// into place.
    fn paste_notes_at_cursor(&mut self, def_id: i32) {
        if self.clipboard.is_empty() {
            return;
        }
        let Some((cx, cy)) = self.windows.get(&def_id).map(|w| w.cursor) else {
            return;
        };
        let (fb_w, fb_h) = self.fb(def_id);
        let Some(h) = interact::pianoroll_hit(&self.host, def_id, fb_w, fb_h, cx, cy) else {
            return;
        };
        let Some((id, _rect, WidgetKind::PianoRoll { .. })) = self.hit(def_id, cx, cy) else {
            return;
        };
        let nav = View {
            start: h.nav.start,
            len: h.nav.len,
        };
        let at = interact::snap(pianoroll::time_at(h.grid, &nav, 0.0, cx as f32), h.snap);
        let clip = self.clipboard.clone();
        interact::pianoroll_state_edit(&mut self.host, def_id, id, |notes, sel| {
            *sel = pianoroll::paste_notes(notes, &clip, at);
        });
        self.host.sync_track_totals();
        self.emit_notes(def_id, id);
        self.redraw(def_id);
    }

    /// Delete/Backspace: remove every selected note of the piano-roll under the
    /// cursor — the block delete (Ctrl+click removes one). A no-op when the
    /// cursor is elsewhere or nothing is selected.
    fn delete_selected_notes(&mut self, def_id: i32) {
        let Some((cx, cy)) = self.windows.get(&def_id).map(|w| w.cursor) else {
            return;
        };
        let Some((id, _rect, WidgetKind::PianoRoll { .. })) = self.hit(def_id, cx, cy) else {
            return;
        };
        let removed = interact::pianoroll_state_edit(&mut self.host, def_id, id, |notes, sel| {
            if sel.is_empty() {
                return false;
            }
            pianoroll::remove_notes(notes, sel);
            sel.clear();
            true
        })
        .unwrap_or(false);
        if removed {
            self.host.sync_track_totals();
            self.emit_notes(def_id, id);
            self.redraw(def_id);
        }
    }

    fn reset_timelines(&mut self, def_id: i32) {
        let mut ids: Vec<i32> = Vec::new();
        if let Some(ws) = self.windows.get(&def_id) {
            ids.extend(ws.waveforms.keys().copied());
            ids.extend(ws.spectrograms.keys().copied());
        }
        for id in ids {
            // The whole group resets (linked members in other windows too).
            let roots = self.host.reset_timeline(id);
            self.redraw_all(&roots);
            self.emit_view(def_id, id);
            // The reset also restores the full vertical axis (and reports it).
            self.set_y_view(def_id, id, 0.0, 1.0);
        }
        self.redraw(def_id);
    }
}
