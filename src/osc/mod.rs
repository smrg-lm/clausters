//! OSC layer: UDP/TCP server, parsing with rosc and translation into engine
//! commands.

pub mod graph;
pub mod graphdef;
pub mod server;
pub mod tcp;
pub mod translate;
pub mod ws;

use std::net::SocketAddr;

use rosc::OscPacket;

/// Where a request came from and where its replies go (M14): the OSC
/// *encoding* is transport-independent, so client identity is too. `Udp` is
/// a remote socket; `Tcp(id)` is a connected TCP client (the per-connection id
/// from `tcp`); `Ring` is the single shared-memory / in-process ring client of
/// `server::ipc`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ClientId {
    Udp(SocketAddr),
    Tcp(u64),
    /// A connected WebSocket client (the per-connection id from `ws`).
    Ws(u64),
    Ring,
}

impl std::fmt::Display for ClientId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientId::Udp(addr) => write!(f, "{addr}"),
            ClientId::Tcp(id) => write!(f, "tcp client {id}"),
            ClientId::Ws(id) => write!(f, "ws client {id}"),
            ClientId::Ring => write!(f, "ring client"),
        }
    }
}

/// Decodes one OSC packet — the single decode entry point every transport
/// funnels through (UDP datagrams and IPC ring contents alike), so decoding and
/// any future hardening live in one place. Delegates to
/// [`clausters_core::osc::decode_packet`], the door shared with every client
/// (the GUI host included), so there is one decoder across the whole system.
pub fn decode_packet(bytes: &[u8]) -> Result<OscPacket, String> {
    clausters_core::osc::decode_packet(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rosc::{OscBundle, OscMessage, OscTime, OscType, encoder};

    /// A blob whose length is a multiple of 4 round-trips — at the top level
    /// and as an element inside a bundle.
    #[test]
    fn multiple_of_four_blob_round_trips() {
        let blob = || OscType::Blob(vec![1, 2, 3, 4]);
        let msg = OscPacket::Message(OscMessage {
            addr: "/b".into(),
            args: vec![blob()],
        });
        let bytes = encoder::encode(&msg).unwrap();
        let OscPacket::Message(m) = decode_packet(&bytes).unwrap() else {
            panic!("expected a message");
        };
        assert_eq!(m.args, vec![blob()]);

        let bundle = OscPacket::Bundle(OscBundle {
            timetag: OscTime {
                seconds: 0,
                fractional: 1,
            },
            content: vec![msg],
        });
        let bytes = encoder::encode(&bundle).unwrap();
        let OscPacket::Bundle(b) = decode_packet(&bytes).unwrap() else {
            panic!("expected a bundle");
        };
        assert_eq!(b.content.len(), 1, "the blob element must not be dropped");
    }
}
