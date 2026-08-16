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
//!
//! The module is split by role, all methods on the one [`app::App`]:
//! [`app`] owns the state and the winit handler, [`windows`] the window
//! lifecycle, [`serverleg`] the audio-server client leg, [`input`] the thin
//! adapters onto the shared gesture machine ([`crate::host::gestures`], which
//! owns all interaction logic), and `midi` the live MIDI note painting — in
//! backticks because that one is behind a feature, and a link into a module a
//! configuration compiles out resolves in one build and not the next.

mod app;
mod input;
#[cfg(feature = "midi")]
mod midi;
mod serverleg;
mod windows;

use std::net::{Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};
use winit::event_loop::{ControlFlow, EventLoop, EventLoopProxy};

use super::{BusSource, ClientId, Host};
use app::App;

/// Repaint period for windows with live (shared-memory-backed) widgets — ~30 fps,
/// enough for smooth meters/scopes without spinning the CPU.
const FRAME: Duration = Duration::from_millis(33);
/// How often a window with a `nodetree` re-queries the server's tree. Node
/// creation/removal is caught immediately through `/node_start`/`/node_end`; this low-rate
/// poll picks up `/node_set` control changes (which raise no notification).
const NODETREE_POLL: Duration = Duration::from_millis(200);

/// The origin of a window with no script behind it (a standalone's pre-loaded
/// GuiDef): a UDP port-0 placeholder no reply is ever sent to.
const PLACEHOLDER_ORIGIN: ClientId = ClientId::Udp(SocketAddr::new(
    std::net::IpAddr::V4(Ipv4Addr::LOCALHOST),
    0,
));

/// What the background transport threads hand the main (winit) thread.
#[derive(Debug)]
pub enum UserEvent {
    /// One OSC datagram from a script and where it came from (decoded on the main
    /// thread, through the single shared door, to keep all logic on one thread).
    Osc { from: SocketAddr, bytes: Vec<u8> },
    /// A new TCP connection on the script front: its id and the write
    /// half its replies go out through. The reader threads feed the event loop
    /// directly (no wake datagram needed — the proxy *is* the wake).
    TcpConnected { id: u64, stream: TcpStream },
    /// One framed OSC packet from TCP connection `id`.
    TcpOsc { id: u64, bytes: Vec<u8> },
    /// TCP connection `id` closed; its write half is dropped.
    TcpDisconnected { id: u64 },
    /// A new WebSocket connection on the script front (`--ws`): its id, the
    /// channel its replies are queued through (the connection thread writes
    /// them — a tungstenite socket owns both halves) and the raw handle an
    /// overflowing reply force-drops it with.
    WsConnected {
        id: u64,
        reply: std::sync::mpsc::SyncSender<Vec<u8>>,
        raw: TcpStream,
    },
    /// One OSC packet (a binary message) from WebSocket connection `id`.
    WsOsc { id: u64, bytes: Vec<u8> },
    /// WebSocket connection `id` closed; its reply channel is dropped.
    WsDisconnected { id: u64 },
    /// One OSC reply from the audio server (the client leg): `/buffer_query.reply`, `/buffer_getRange.reply`.
    ServerOsc { bytes: Vec<u8> },
}

/// Runs the windowed host: spawn the transport thread(s), then own the winit
/// event loop on this (main) thread until the process is stopped.
///
/// `bus` is the shared-memory data plane read each frame for meters, scopes and
/// the playhead; `None` leaves those views reading zero. It arrives **already
/// built** rather than as a path because there is more than one way to get one
/// — [`open_shm`] maps a separate server's `--shm` file, and an embedded server
/// hands over its own in-memory segment — and because *which counter a playhead
/// reads* is settled where the source is made, not here.
///
/// `tcp` is the script front's TCP carrier — `(port, max_frame)` — bound here
/// because its reader threads feed the event loop through its proxy (no wake
/// datagram: the proxy is the wake); `None` leaves the front UDP-only.
pub fn run(
    host: Host,
    socket: Arc<UdpSocket>,
    bus: Option<Arc<dyn BusSource>>,
    tcp: Option<(u16, usize)>,
    ws: Option<(u16, usize)>,
) -> Result<(), String> {
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
    // The framed TCP leg of the same front, straight into the event loop.
    if let Some((port, max_frame)) = tcp {
        let tcp_proxy = proxy.clone();
        let bound = super::tcp::bind_with_sink(("127.0.0.1", port), max_frame, move |event| {
            let user_event = match event {
                super::tcp::TcpEvent::Connected(id, stream) => {
                    UserEvent::TcpConnected { id, stream }
                }
                super::tcp::TcpEvent::Frame(id, bytes) => UserEvent::TcpOsc { id, bytes },
                super::tcp::TcpEvent::Disconnected(id) => UserEvent::TcpDisconnected { id },
            };
            tcp_proxy.send_event(user_event).is_ok()
        })
        .map_err(|e| format!("failed to bind TCP port {port}: {e}"))?;
        tracing::info!("clausters-gui host listening on tcp://{bound} (script -> host)");
    }
    // The WebSocket leg (`--ws`), the browser's carrier — same shape as TCP:
    // the connection threads feed the event loop through its proxy.
    if let Some((port, max_frame)) = ws {
        let ws_proxy = proxy.clone();
        let bound = super::ws::bind_with_sink(("0.0.0.0", port), max_frame, move |event| {
            let user_event = match event {
                super::ws::WsEvent::Connected(id, reply, raw) => {
                    UserEvent::WsConnected { id, reply, raw }
                }
                super::ws::WsEvent::Frame(id, bytes) => UserEvent::WsOsc { id, bytes },
                super::ws::WsEvent::Disconnected(id) => UserEvent::WsDisconnected { id },
            };
            ws_proxy.send_event(user_event).is_ok()
        })
        .map_err(|e| format!("failed to bind WebSocket port {port}: {e}"))?;
        tracing::info!(
            "clausters-gui host listening on ws://{bound} (script -> host, browser-reachable)"
        );
    }
    // The host <- audio-server reply path: a background thread only for the UDP
    // leg (the embed link is polled in the event loop, no socket to drain).
    if let Some(leg_socket) = host.server().and_then(|s| s.udp_socket()) {
        std::thread::Builder::new()
            .name("clausters-gui-server".into())
            .spawn(move || server_reply_loop(leg_socket, proxy))
            .map_err(|e| e.to_string())?;
    }

    let mut app = App::new(host, socket, bus);
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
/// main thread (which routes `/buffer_query.reply`/`/buffer_getRange.reply` into the buffer-fetch path).
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
pub fn open_shm(path: Option<String>) -> Option<Arc<dyn BusSource>> {
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
pub fn open_shm(path: Option<String>) -> Option<Arc<dyn BusSource>> {
    if path.is_some() {
        warn!("--shm (shared-memory meters) is only supported on Unix");
    }
    None
}
