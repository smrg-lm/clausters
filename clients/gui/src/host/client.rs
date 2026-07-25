//! The host's client leg: `clausters-gui` as a client of the audio server.
//!
//! The third leg of the topology. The host reads buffers/buses/the node tree
//! and sends control to `clausters-server` exactly as the Python client does —
//! and, crucially, **through the same OSC encode door**
//! ([`clausters_core::osc`]), so there is one encoder across the system, not a
//! parallel one. The leg is bidirectional: the host **sends** queries/control
//! (`/b_query`, `/b_getn`, later `/n_set` for bound widgets) and **receives**
//! the server's replies (`/b_info`, `/b_setn`). The windowed front pumps those
//! replies into the main thread to fill server-buffer waveform views.
//!
//! Control buses are *not* read through this leg: a `meter`/`scope` reads them
//! directly from the shared-memory segment ([`super::shm`]) with no messages at
//! all. The leg carries only what shared memory cannot — the command plane.

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;

use clausters_core::osc::{OscMessage, OscPacket, encode};

/// A UDP connection to the audio server, used to send it OSC control and read
/// its replies. The socket is shared (`Arc`) so the windowed front can spawn a
/// receive thread on a clone while the main thread keeps sending.
pub struct ServerLeg {
    socket: Arc<UdpSocket>,
    target: SocketAddr,
}

impl ServerLeg {
    /// Opens the leg toward the audio server at `target` (an ephemeral local
    /// port so the server's replies have somewhere to return).
    pub fn connect(target: SocketAddr) -> io::Result<Self> {
        let socket = UdpSocket::bind(("127.0.0.1", 0))?;
        Ok(Self {
            socket: Arc::new(socket),
            target,
        })
    }

    /// The audio server this leg targets.
    pub fn target(&self) -> SocketAddr {
        self.target
    }

    /// A clone of the underlying socket, for a background receive thread.
    pub fn socket(&self) -> Arc<UdpSocket> {
        Arc::clone(&self.socket)
    }

    /// Sends one OSC message to the audio server.
    pub fn send(&self, msg: OscMessage) -> io::Result<()> {
        let bytes = encode(&OscPacket::Message(msg))
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        self.socket.send_to(&bytes, self.target)?;
        Ok(())
    }
}
