//! TCP transport for the OSC server.
//!
//! The command loop ([`super::server::OscServer::run`]) is single-threaded and
//! blocks on the UDP socket; TCP is multiplexed in **without an async runtime
//! or a new dependency**, mirroring the IPC-ring pattern. A small acceptor
//! thread plus one reader thread per connection turn each byte stream into
//! whole, length-prefixed OSC frames and hand them to the loop over an
//! [`mpsc`](std::sync::mpsc) channel; the loop owns each connection's write
//! half for replies. A **zero-length UDP datagram** to the server's own address
//! wakes the loop the instant a frame (or a disconnect) is queued, so a TCP
//! request is served without waiting for the periodic GC tick.
//!
//! Wire framing (same as scsynth's TCP): a 4-byte big-endian length prefix
//! followed by exactly that many OSC bytes — a message or a bundle, decoded
//! through the single [`super::decode_packet`] door like every other transport.
//! Replies use the same framing.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::Duration;

use super::ClientSlots;

/// Capacity, in frames, of the bounded channel from the reader threads to the
/// command loop. When a client floods frames faster than the loop drains
/// them, its reader blocks here and TCP flow control pushes back to the
/// sender — bounding server memory instead of growing an unbounded queue.
/// Sized in frames (not bytes): plenty for dense small-message control, while
/// the worst case stays `INBOUND_QUEUE × max_frame`.
const INBOUND_QUEUE: usize = 256;

/// How long a reply write may block before the connection is dropped. Replies
/// are written by the command loop itself, so a client that stops reading
/// (with a full socket buffer) would otherwise stall the whole server; the
/// timeout bounds that stall and evicts the slow consumer. Generous — it
/// fires only when the client makes no progress at all for this long.
const REPLY_WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// What a reader thread hands the command loop.
enum TcpEvent {
    /// A new connection: its id and the write half for replies.
    Connected(u64, TcpStream),
    /// A complete OSC frame from connection `id`.
    Frame(u64, Vec<u8>),
    /// Connection `id` closed (EOF or error).
    Disconnected(u64),
}

/// The server side of the TCP transport: the event stream the loop drains and
/// the connection write halves it replies through. The `TcpListener` lives in
/// the acceptor thread; dropping the hub drops the channel, which makes the
/// reader threads exit on their next send.
pub struct TcpHub {
    events: Receiver<TcpEvent>,
    /// Write halves by connection id (a clone of the accepted stream). Writing
    /// goes through `&TcpStream`, which implements [`Write`], so a reply needs
    /// only a shared borrow; dead connections are pruned on `Disconnected`.
    conns: HashMap<u64, TcpStream>,
    /// Connection ids whose `Disconnected` went by since the last
    /// [`take_disconnects`](Self::take_disconnects), so the command loop can
    /// drop per-client state (bus streams, `/server_notify` registrations).
    disconnects: Vec<u64>,
    local_addr: SocketAddr,
}

impl TcpHub {
    /// Binds a TCP listener on `addr` and starts accepting. `wake_target` is the
    /// server's own UDP address; reader threads send a zero-length datagram
    /// there to wake the command loop when a frame or disconnect is queued.
    /// `max_frame` is the largest OSC frame accepted on a connection — a
    /// prefix above it (or a zero prefix) closes the connection instead of
    /// allocating on an untrusted length (see
    /// [`super::DEFAULT_MAX_FRAME`]). `slots` is the live-client ceiling
    /// (`--max-clients`), shared with the WebSocket front; a connection past
    /// it is dropped at accept.
    pub fn bind(
        addr: impl ToSocketAddrs,
        wake_target: SocketAddr,
        max_frame: usize,
        slots: Arc<ClientSlots>,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        let local_addr = listener.local_addr()?;
        let (tx, rx) = sync_channel(INBOUND_QUEUE);
        // A throwaway UDP socket the reader threads use only to send wake bytes.
        let wake = Arc::new(UdpSocket::bind(("127.0.0.1", 0))?);
        std::thread::Builder::new()
            .name("clausters-tcp-accept".into())
            .spawn(move || accept_loop(listener, tx, wake, wake_target, max_frame, slots))
            .expect("failed to spawn the TCP acceptor thread");
        Ok(Self {
            events: rx,
            conns: HashMap::new(),
            disconnects: Vec::new(),
            local_addr,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// The next complete frame `(connection id, bytes)`, or `None` when the
    /// queue is drained. Registers and forgets connections as their
    /// `Connected`/`Disconnected` events go by — both precede / follow that
    /// connection's frames in the channel, so the write half is always present
    /// before a frame is returned for handling.
    pub fn next_frame(&mut self) -> Option<(u64, Vec<u8>)> {
        while let Ok(event) = self.events.try_recv() {
            match event {
                TcpEvent::Connected(id, write_half) => {
                    self.conns.insert(id, write_half);
                }
                TcpEvent::Disconnected(id) => {
                    self.conns.remove(&id);
                    self.disconnects.push(id);
                }
                TcpEvent::Frame(id, bytes) => return Some((id, bytes)),
            }
        }
        None
    }

    /// Connection ids that disconnected since the last call. The command loop
    /// drains this after [`next_frame`](Self::next_frame) returns `None` to
    /// forget any per-client state it holds for them.
    pub fn take_disconnects(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.disconnects)
    }

    /// Writes a length-prefixed reply to connection `id` (silently dropped if
    /// the connection is gone — the `Disconnected` event prunes it). A write
    /// that fails — including one that stalls past `REPLY_WRITE_TIMEOUT`
    /// because the client stopped reading — drops the connection: its reader
    /// sees the shutdown and the `Disconnected` event prunes the state.
    pub fn reply(&self, id: u64, bytes: &[u8]) {
        if let Some(stream) = self.conns.get(&id)
            && let Err(e) = write_frame(stream, bytes)
        {
            tracing::warn!("dropping tcp client {id}: reply failed: {e}");
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

fn accept_loop(
    listener: TcpListener,
    tx: SyncSender<TcpEvent>,
    wake: Arc<UdpSocket>,
    wake_target: SocketAddr,
    max_frame: usize,
    slots: Arc<ClientSlots>,
) {
    let next_id = AtomicU64::new(1);
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let Some(slot) = slots.try_acquire() else {
            tracing::warn!("refusing tcp connection: --max-clients ceiling reached");
            continue; // dropping the stream closes it
        };
        let _ = stream.set_nodelay(true);
        // The dup'd write half shares the socket, so the write timeout set
        // here bounds the command loop's reply writes (see `reply`).
        let _ = stream.set_write_timeout(Some(REPLY_WRITE_TIMEOUT));
        let Ok(write_half) = stream.try_clone() else {
            continue;
        };
        let id = next_id.fetch_add(1, Ordering::Relaxed);
        // Register the connection before its reader can produce any frame.
        if tx.send(TcpEvent::Connected(id, write_half)).is_err() {
            return; // the loop (and hub) are gone
        }
        let tx = tx.clone();
        let wake = Arc::clone(&wake);
        std::thread::Builder::new()
            .name(format!("clausters-tcp-{id}"))
            .spawn(move || {
                // The slot rides the reader thread and frees on any exit.
                let _slot = slot;
                reader_loop(id, stream, tx, &wake, wake_target, max_frame)
            })
            .ok();
    }
}

fn reader_loop(
    id: u64,
    mut stream: TcpStream,
    tx: SyncSender<TcpEvent>,
    wake: &UdpSocket,
    wake_target: SocketAddr,
    max_frame: usize,
) {
    let mut prefix = [0u8; 4];
    while stream.read_exact(&mut prefix).is_ok() {
        let len = u32::from_be_bytes(prefix) as usize;
        if len == 0 || len > max_frame {
            break; // protocol violation: drop the connection
        }
        let mut frame = vec![0u8; len];
        if stream.read_exact(&mut frame).is_err() {
            break;
        }
        if tx.send(TcpEvent::Frame(id, frame)).is_err() {
            return;
        }
        let _ = wake.send_to(&[], wake_target);
    }
    let _ = tx.send(TcpEvent::Disconnected(id));
    let _ = wake.send_to(&[], wake_target);
}

/// Writes `bytes` framed with a 4-byte big-endian length prefix. `&TcpStream`
/// implements [`Write`], so this takes a shared borrow.
fn write_frame(mut w: impl Write, bytes: &[u8]) -> io::Result<()> {
    let len = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "OSC frame too large for TCP"))?;
    w.write_all(&len.to_be_bytes())?;
    w.write_all(bytes)?;
    w.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `--max-clients` ceiling: with one slot, the first connection is
    /// served, the second is dropped at accept, and closing the first frees
    /// its slot for a third. Runs entirely in-process (client and hub in one
    /// test), so localhost sockets are reachable.
    #[test]
    fn max_clients_ceiling_drops_and_recycles() {
        let wake = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let wake_addr = wake.local_addr().unwrap();
        let slots = Arc::new(ClientSlots::new(1));
        let mut hub = TcpHub::bind(
            ("127.0.0.1", 0),
            wake_addr,
            crate::osc::DEFAULT_MAX_FRAME,
            slots,
        )
        .unwrap();
        let addr = hub.local_addr();

        let send_ping = |stream: &TcpStream| write_frame(stream, b"/png").unwrap();
        let is_closed = |stream: &mut TcpStream| {
            // The acceptor never announces the refused stream; the drop
            // surfaces to the client as EOF (or a reset) on its next read.
            let mut buf = [0u8; 1];
            matches!(stream.read(&mut buf), Ok(0) | Err(_))
        };
        let wait_frame = |hub: &mut TcpHub| {
            for _ in 0..200 {
                if let Some(frame) = hub.next_frame() {
                    return frame;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("no frame arrived within the deadline");
        };

        // First connection: in, and its frames flow.
        let first = TcpStream::connect(addr).unwrap();
        send_ping(&first);
        let (first_id, bytes) = wait_frame(&mut hub);
        assert_eq!(bytes, b"/png");

        // Second connection: past the ceiling, dropped without being served.
        // The write may already fail with a reset, which is the drop showing.
        let mut second = TcpStream::connect(addr).unwrap();
        let _ = write_frame(&second, b"/png");
        assert!(
            is_closed(&mut second),
            "the second connection must be dropped"
        );

        // Closing the first frees its slot; a third connection is served. The
        // slot releases when the first reader thread exits, a moment after
        // the disconnect event, so the connection attempt retries.
        drop(first);
        let mut third_served = false;
        'attempts: for _ in 0..100 {
            let third = TcpStream::connect(addr).unwrap();
            let _ = write_frame(&third, b"/png");
            for _ in 0..10 {
                if let Some((id, bytes)) = hub.next_frame() {
                    assert_eq!(bytes, b"/png");
                    assert_ne!(id, first_id);
                    third_served = true;
                    break 'attempts;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        assert!(third_served, "a freed slot must serve a new connection");
    }
}
