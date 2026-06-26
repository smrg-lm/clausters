//! The host's server front: the transport carrying the `/gui_*` protocol from
//! the script to the host.
//!
//! A thin UDP front for this milestone (see the transport decision in the
//! module docs): bind a socket, decode each datagram through the single shared
//! [`clausters_core::osc::decode_packet`] door, hand it to [`Host`], and send
//! the reply messages back to the requester. [`ClientId`] names where a request
//! came from and where its replies go — UDP only here, with TCP/WebSocket/ring
//! variants to follow behind the same seam, mirroring the audio server's own
//! `ClientId`.

use std::io;
use std::net::{SocketAddr, UdpSocket};

use clausters_core::osc::{OscMessage, OscPacket, encode};
use tracing::{info, warn};

use super::{Host, HostEffect};

/// The default UDP port for the GUI host's server front. Chosen clear of the
/// audio server's family (UDP/TCP 57110, WebSocket 57120) so both can run on
/// one machine without colliding.
pub const DEFAULT_PORT: u16 = 57210;

/// Where a request reached the host and where its replies go. The `/gui_*`
/// *encoding* is transport-independent, so client identity is too — UDP here,
/// the other carriers added in later milestones.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ClientId {
    Udp(SocketAddr),
}

impl std::fmt::Display for ClientId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientId::Udp(addr) => write!(f, "{addr}"),
        }
    }
}

/// Runs the **headless** UDP server front until an unrecoverable socket error:
/// receive a datagram, decode it, let `host` interpret it, send each reply back
/// to the sender. Single-threaded, like the audio server's command loop. Window
/// effects are logged rather than acted on — this front has no display (the
/// windowed front lives in [`super::gui`]); it is for tests, automation and
/// no-display environments.
pub fn serve(mut host: Host, socket: UdpSocket) -> io::Result<()> {
    let mut buf = vec![0u8; 65536];
    loop {
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
            continue; // a zero-length wake datagram (used by future transports)
        }
        let packet = match clausters_core::osc::decode_packet(&buf[..len]) {
            Ok(packet) => packet,
            Err(e) => {
                warn!("malformed OSC packet from {from}: {e}");
                continue;
            }
        };
        for effect in host.handle_packet(packet, ClientId::Udp(from)) {
            match effect {
                HostEffect::Reply(msg) => send_reply(&socket, from, msg),
                HostEffect::OpenWindow(id) => {
                    info!("gui_def {id}: window requested (headless front: not opening a window)")
                }
                HostEffect::CloseWindow(id) => info!("gui_free {id}: window closed (headless)"),
                HostEffect::Redraw(_) => {} // nothing to repaint headless
            }
        }
    }
}

fn send_reply(socket: &UdpSocket, to: SocketAddr, msg: OscMessage) {
    let addr = msg.addr.clone();
    let bytes = match encode(&OscPacket::Message(msg)) {
        Ok(bytes) => bytes,
        Err(e) => return warn!("failed to encode {addr}: {e}"),
    };
    if let Err(e) = socket.send_to(&bytes, to) {
        warn!("failed to send {addr} to {to}: {e}");
    }
}
