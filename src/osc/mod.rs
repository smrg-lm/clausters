//! OSC layer: UDP/TCP server, parsing with rosc and translation into engine
//! commands.

pub mod graph;
pub mod graphdef;
pub mod server;
pub mod tcp;
pub mod translate;
// The WebSocket hub rides tungstenite, which cannot build for wasm32 (and an
// in-page engine has no use for a socket server front) — native only.
#[cfg(not(target_arch = "wasm32"))]
pub mod ws;

use std::net::SocketAddr;

use rosc::OscPacket;

/// Where a request came from and where its replies go: the OSC
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

/// The stream-transport frame ceiling's default, shared with the GUI host
/// through the core so both ends of the wire agree (see the constant's own
/// docs for the rationale).
pub use clausters_core::osc::DEFAULT_MAX_FRAME;

/// Default ceiling for concurrent stream clients (TCP + WebSocket combined,
/// `--max-clients`). Each connection costs a thread and queue slots, so the
/// count is bounded like every other boot-time pool — a DoS guard in the
/// spirit of scsynth's `maxLogins`, sized generously for the target
/// deployments (a session rarely holds more than a handful of clients). UDP
/// is connectionless and unaffected.
pub const DEFAULT_MAX_CLIENTS: usize = 64;

/// Live stream-client slots, shared by the TCP and WebSocket acceptors so the
/// `--max-clients` ceiling bounds both fronts together. An acceptor takes a
/// slot per connection ([`try_acquire`](Self::try_acquire)) and the returned
/// guard gives it back when the connection's thread exits.
pub struct ClientSlots {
    live: std::sync::atomic::AtomicUsize,
    max: usize,
}

impl ClientSlots {
    pub fn new(max: usize) -> Self {
        Self {
            live: std::sync::atomic::AtomicUsize::new(0),
            max,
        }
    }

    /// Claims a slot, or `None` when the ceiling is reached (the acceptor
    /// drops the connection). The guard releases the slot on drop, covering
    /// every exit path of a connection thread.
    pub fn try_acquire(self: &std::sync::Arc<Self>) -> Option<SlotGuard> {
        use std::sync::atomic::Ordering;
        self.live
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < self.max).then_some(n + 1)
            })
            .ok()
            .map(|_| SlotGuard(std::sync::Arc::clone(self)))
    }
}

/// Releases its [`ClientSlots`] slot on drop.
pub struct SlotGuard(std::sync::Arc<ClientSlots>);

impl Drop for SlotGuard {
    fn drop(&mut self) {
        self.0
            .live
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
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
