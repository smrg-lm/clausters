//! The host's server front: the transport carrying the `/gui_*` protocol from
//! the script to the host.
//!
//! Three carriers behind the one [`ClientId`] seam, mirroring the audio
//! server's own: **UDP** (a datagram per packet), **TCP** (length-prefixed
//! frames, [`super::tcp`], on by default — the command plane for payloads a
//! datagram cannot carry, a whole `/gui_def` tree first among them) and
//! **WebSocket** ([`super::ws`], opt-in with `--ws` — one OSC packet per
//! binary message, the browser's carrier into a native host). Every inbound
//! byte string decodes through the single shared
//! [`clausters_core::osc::decode_packet`] door, is handed to [`Host`], and the
//! replies go back to the requester over the transport it came in on. The
//! serve loop blocks on the UDP socket; a TCP/WS reader thread wakes it with a
//! **zero-length datagram** to the host's own address the moment a frame is
//! queued, the audio server's multiplexing pattern.

use std::io;
use std::net::{SocketAddr, UdpSocket};

use clausters_core::osc::{OscMessage, OscPacket, encode};
use tracing::{info, warn};

use super::tcp::TcpHub;
use super::ws::WsHub;
use super::{ClientId, Host, HostEffect};

/// The default port for the GUI host's server front (UDP and TCP alike).
/// Chosen clear of the audio server's family (UDP/TCP 57110, WebSocket 57120)
/// so both can run on one machine without colliding.
pub const DEFAULT_PORT: u16 = 57210;

/// Binds the script front's TCP leg on `port` for the headless loop: the
/// hub's reader threads wake `socket` (the front's own UDP socket) with a
/// zero-length datagram whenever a frame is queued.
pub fn bind_tcp(socket: &UdpSocket, port: u16, max_frame: usize) -> io::Result<TcpHub> {
    TcpHub::bind(("127.0.0.1", port), wake_target(socket)?, max_frame)
}

/// Binds the script front's WebSocket leg on `port` for the headless loop —
/// same wake pattern as TCP. Bound like the audio server's `--ws` (reachable
/// beyond loopback: a browser on another machine is the point).
pub fn bind_ws(socket: &UdpSocket, port: u16, max_frame: usize) -> io::Result<WsHub> {
    WsHub::bind(("0.0.0.0", port), wake_target(socket)?, max_frame)
}

fn wake_target(socket: &UdpSocket) -> io::Result<SocketAddr> {
    let mut wake_target = socket.local_addr()?;
    if wake_target.ip().is_unspecified() {
        wake_target.set_ip(match wake_target {
            SocketAddr::V4(_) => std::net::Ipv4Addr::LOCALHOST.into(),
            SocketAddr::V6(_) => std::net::Ipv6Addr::LOCALHOST.into(),
        });
    }
    Ok(wake_target)
}

/// Runs the **headless** server front until an unrecoverable socket error:
/// receive a packet (a UDP datagram, or a framed TCP request drained after the
/// wake datagram), decode it, let `host` interpret it, send each reply back
/// to the sender over its own transport. Single-threaded, like the audio
/// server's command loop. Window effects are logged rather than acted on —
/// this front has no display (the windowed front lives in [`super::gui`]); it
/// is for tests, automation and no-display environments.
pub fn serve(
    mut host: Host,
    socket: UdpSocket,
    mut tcp: Option<TcpHub>,
    mut ws: Option<WsHub>,
) -> io::Result<()> {
    let mut buf = vec![0u8; 65536];
    loop {
        // Framed TCP/WS requests first: the wake datagram only says "look".
        while let Some((id, bytes)) = tcp.as_mut().and_then(|hub| hub.next_frame()) {
            handle(&mut host, &bytes, ClientId::Tcp(id), &socket, &tcp, &ws);
        }
        while let Some((id, bytes)) = ws.as_mut().and_then(|hub| hub.next_frame()) {
            handle(&mut host, &bytes, ClientId::Ws(id), &socket, &tcp, &ws);
        }
        let (len, from) = match socket.recv_from(&mut buf) {
            Ok(ok) => ok,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
            Err(e) if e.kind() == io::ErrorKind::TimedOut => continue,
            // A previous reply to a now-closed client port can surface as
            // ECONNREFUSED on the next recv (Linux); not fatal.
            Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => continue,
            Err(e) => return Err(e),
        };
        if len == 0 {
            continue; // a zero-length wake datagram: loop back to drain TCP/WS
        }
        let bytes = buf[..len].to_vec();
        handle(&mut host, &bytes, ClientId::Udp(from), &socket, &tcp, &ws);
    }
}

/// Decodes one packet, runs it through the host, and dispatches the effects —
/// replies routed by the requester's `ClientId`.
fn handle(
    host: &mut Host,
    bytes: &[u8],
    from: ClientId,
    socket: &UdpSocket,
    tcp: &Option<TcpHub>,
    ws: &Option<WsHub>,
) {
    let packet = match clausters_core::osc::decode_packet(bytes) {
        Ok(packet) => packet,
        Err(e) => return warn!("malformed OSC packet from {from}: {e}"),
    };
    for effect in host.handle_packet(packet, from) {
        match effect {
            HostEffect::Reply(msg) => send_reply(socket, tcp, ws, from, msg),
            HostEffect::OpenWindow(id) => {
                info!("gui_def {id}: window requested (headless front: not opening a window)")
            }
            HostEffect::CloseWindow(id) => info!("gui_free {id}: window closed (headless)"),
            HostEffect::Redraw(_) => {} // nothing to repaint headless
        }
    }
}

fn send_reply(
    socket: &UdpSocket,
    tcp: &Option<TcpHub>,
    ws: &Option<WsHub>,
    to: ClientId,
    msg: OscMessage,
) {
    let addr = msg.addr.clone();
    let bytes = match encode(&OscPacket::Message(msg)) {
        Ok(bytes) => bytes,
        Err(e) => return warn!("failed to encode {addr}: {e}"),
    };
    match to {
        ClientId::Udp(to) => {
            if let Err(e) = socket.send_to(&bytes, to) {
                warn!("failed to send {addr} to {to}: {e}");
            }
        }
        ClientId::Tcp(id) => {
            if let Some(hub) = tcp {
                hub.reply(id, &bytes);
            }
        }
        ClientId::Ws(id) => {
            if let Some(hub) = ws {
                hub.reply(id, &bytes);
            }
        }
        // The wasm front never reaches the native serve loop.
        ClientId::Web => warn!("reply {addr} to a web client on the native front"),
    }
}
