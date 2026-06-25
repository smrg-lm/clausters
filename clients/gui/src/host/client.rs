//! The host's client leg: `clausters-gui` as a client of the audio server.
//!
//! The third leg of the topology. The host reads buffers/buses/the node tree
//! and sends control to `clausters-server` exactly as the Python client does —
//! and, crucially, **through the same OSC encode door**
//! ([`clausters_core::osc`]), so there is one encoder across the system, not a
//! parallel one. This milestone scaffolds the leg (a UDP sender to the audio
//! server); the features that use it — buffer reads and shared-memory meters
//! (G5), bound-widget forwarding (G6) — build on top.

use std::io;
use std::net::{SocketAddr, UdpSocket};

use clausters_core::osc::{OscMessage, OscPacket, encode};

/// A UDP connection to the audio server, used to send it OSC control.
pub struct ServerLeg {
    socket: UdpSocket,
    target: SocketAddr,
}

impl ServerLeg {
    /// Opens the leg toward the audio server at `target` (an ephemeral local
    /// port so the server's replies have somewhere to return).
    pub fn connect(target: SocketAddr) -> io::Result<Self> {
        let socket = UdpSocket::bind(("127.0.0.1", 0))?;
        Ok(Self { socket, target })
    }

    /// The audio server this leg targets.
    pub fn target(&self) -> SocketAddr {
        self.target
    }

    /// Sends one OSC message to the audio server.
    pub fn send(&self, msg: OscMessage) -> io::Result<()> {
        let bytes = encode(&OscPacket::Message(msg))
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        self.socket.send_to(&bytes, self.target)?;
        Ok(())
    }
}
