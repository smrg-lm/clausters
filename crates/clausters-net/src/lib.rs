//! The stream transports: OSC over TCP and over WebSocket, written once.
//!
//! The audio server and the GUI host both serve OSC on a UDP socket and on two
//! stream carriers beside it, and both run a single-threaded command loop that
//! must never block on a client. This crate is the part they share: an
//! acceptor thread plus one thread per connection turn each stream into whole
//! OSC packets, and hand them over as [`Event`]s -- the connection, its
//! frames, its end -- to a **sink** the caller supplies.
//!
//! Two sinks exist. [`Hub`] is the one a command loop drains: a bounded queue
//! plus a [`Waker`], the zero-length datagram to the loop's own UDP address that
//! ends its blocking recv the instant a frame is queued. The GUI host's
//! windowed front supplies its own sink instead -- the window event loop's
//! proxy, which needs no wake at all.
//!
//! What every connection gets, whichever sink it feeds:
//!
//! - **a frame ceiling** (`max_frame`): a larger frame closes the connection
//!   instead of allocating on an untrusted length;
//! - **a client ceiling** ([`ClientSlots`]): a connection past it is dropped at
//!   accept, and the slot is released when the connection's thread exits;
//! - **a bounded reply path**: a TCP reply write gives up after
//!   [`tcp::REPLY_WRITE_TIMEOUT`], a WebSocket reply queue holds
//!   `ws::REPLY_QUEUE` replies, and a client that stops reading is dropped
//!   rather than stalling the loop that answers it.
//!
//! Decoding is not here: the bytes of a frame go to the caller, which decodes
//! them through its one `decode_packet` door like every other transport's.

use std::collections::HashMap;
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

pub mod tcp;
#[cfg(not(target_arch = "wasm32"))]
pub mod ws;

/// Default ceiling for concurrent stream clients (TCP and WebSocket combined).
/// Each connection costs a thread and queue slots, so the count is bounded like
/// every other boot-time pool -- a guard against a client that opens
/// connections without end, sized generously (a session rarely holds more than
/// a handful of clients). UDP is connectionless and unaffected.
pub const DEFAULT_MAX_CLIENTS: usize = 64;

/// Capacity, in frames, of a [`Hub`]'s queue from the connection threads to the
/// command loop. When a client floods frames faster than the loop drains them,
/// its thread blocks here and TCP flow control pushes back to the sender --
/// bounding memory instead of growing an unbounded queue. Sized in frames (not
/// bytes): plenty for dense small-message control, while the worst case stays
/// `INBOUND_QUEUE * max_frame`.
pub const INBOUND_QUEUE: usize = 256;

/// Live stream-client slots, shared by the TCP and WebSocket acceptors so one
/// ceiling bounds both carriers together. An acceptor takes a slot per
/// connection ([`try_acquire`](Self::try_acquire)) and the returned guard gives
/// it back when the connection's thread exits.
pub struct ClientSlots {
    live: AtomicUsize,
    max: usize,
}

impl ClientSlots {
    pub fn new(max: usize) -> Self {
        Self {
            live: AtomicUsize::new(0),
            max,
        }
    }

    /// Claims a slot, or `None` when the ceiling is reached (the acceptor
    /// drops the connection). The guard releases the slot on drop, covering
    /// every exit path of a connection thread.
    pub fn try_acquire(self: &Arc<Self>) -> Option<SlotGuard> {
        self.live
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < self.max).then_some(n + 1)
            })
            .ok()
            .map(|_| SlotGuard(Arc::clone(self)))
    }
}

/// Releases its [`ClientSlots`] slot on drop.
pub struct SlotGuard(Arc<ClientSlots>);

impl Drop for SlotGuard {
    fn drop(&mut self) {
        self.0.live.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A handle a worker thread pokes to end a command loop's blocking recv.
///
/// The loop blocks in `recv_from` on its UDP socket under a read timeout, so
/// anything produced *off* its thread -- a stream frame, a MIDI message, a
/// finished job -- is only seen when that recv returns. A **zero-length
/// datagram** to the loop's own address ends it immediately: the loop's idle
/// tick stays a housekeeping interval instead of doubling as the latency of
/// every result.
///
/// Cheap to clone (one socket shared by every clone), and silent on failure:
/// a wake that does not arrive costs one idle tick of latency, never a
/// result, so there is nothing useful to report from a worker thread.
#[derive(Clone)]
pub struct Waker {
    socket: Arc<UdpSocket>,
    target: SocketAddr,
}

impl Waker {
    /// Opens a throwaway sender aimed at `target`, the address the command
    /// loop's own socket is bound to ([`loopback`] reads an unspecified bind
    /// address as the loopback one).
    pub fn to(target: SocketAddr) -> io::Result<Self> {
        let bind: SocketAddr = match target {
            SocketAddr::V4(_) => (std::net::Ipv4Addr::LOCALHOST, 0).into(),
            SocketAddr::V6(_) => (std::net::Ipv6Addr::LOCALHOST, 0).into(),
        };
        Ok(Self {
            socket: Arc::new(UdpSocket::bind(bind)?),
            target,
        })
    }

    /// Ends the loop's current recv. Call it *after* the result is queued, so
    /// the woken loop finds it.
    pub fn wake(&self) {
        let _ = self.socket.send_to(&[], self.target);
    }
}

/// `addr` with an unspecified IP read as loopback on the same port: where a
/// datagram to a socket bound on every interface has to be aimed.
pub fn loopback(mut addr: SocketAddr) -> SocketAddr {
    if addr.ip().is_unspecified() {
        addr.set_ip(match addr {
            SocketAddr::V4(_) => std::net::Ipv4Addr::LOCALHOST.into(),
            SocketAddr::V6(_) => std::net::Ipv6Addr::LOCALHOST.into(),
        });
    }
    addr
}

/// What a connection thread hands its sink. `C` is the carrier's reply handle
/// ([`tcp::TcpConn`], `ws::WsConn`).
pub enum Event<C> {
    /// A new connection: its id and the handle its replies go out through.
    /// Sent before any of the connection's frames.
    Connected(u64, C),
    /// A complete OSC frame from connection `id`.
    Frame(u64, Vec<u8>),
    /// Connection `id` closed (EOF, a clean close or an error). Sent after the
    /// last of its frames.
    Disconnected(u64),
}

/// The reply half of one connection.
pub trait Conn: Send + 'static {
    /// Sends `bytes` as one reply. A client that has stopped reading is
    /// dropped rather than waited for; its thread sees the shutdown and sends
    /// [`Event::Disconnected`]. `id` names the connection in the log.
    fn reply(&self, id: u64, bytes: &[u8]);
}

/// A caller's sink: called from the acceptor and connection threads, it returns
/// `false` once the consumer is gone, which stops them.
pub trait Sink<C>: Fn(Event<C>) -> bool + Send + Clone + 'static {}
impl<C, F: Fn(Event<C>) -> bool + Send + Clone + 'static> Sink<C> for F {}

/// The consumer a command loop drains: the event queue and the reply handles
/// of the live connections. The listener lives in the acceptor thread;
/// dropping the hub drops the queue, which makes the connection threads exit
/// on their next send.
pub struct Hub<C> {
    events: Receiver<Event<C>>,
    /// Reply handles by connection id, pruned on `Disconnected`.
    conns: HashMap<u64, C>,
    /// Connection ids whose `Disconnected` went by since the last
    /// [`take_disconnects`](Self::take_disconnects), so the loop can drop what
    /// it holds per client.
    disconnects: Vec<u64>,
    local_addr: SocketAddr,
}

impl<C: Conn> Hub<C> {
    /// A bounded queue whose sender wakes `waker` after every event, and the
    /// hub that drains it. `bind` binds the carrier with that sink and returns
    /// its address.
    fn serve(
        waker: Waker,
        bind: impl FnOnce(SyncSender<Event<C>>, Waker) -> io::Result<SocketAddr>,
    ) -> io::Result<Self> {
        let (tx, rx) = sync_channel(INBOUND_QUEUE);
        let local_addr = bind(tx, waker)?;
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
    /// `Connected`/`Disconnected` events go by -- both bracket that
    /// connection's frames in the queue, so the reply handle is always present
    /// before a frame is returned for handling.
    pub fn next_frame(&mut self) -> Option<(u64, Vec<u8>)> {
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Connected(id, conn) => {
                    self.conns.insert(id, conn);
                }
                Event::Disconnected(id) => {
                    self.conns.remove(&id);
                    self.disconnects.push(id);
                }
                Event::Frame(id, bytes) => return Some((id, bytes)),
            }
        }
        None
    }

    /// Connection ids that disconnected since the last call. The loop drains
    /// this after [`next_frame`](Self::next_frame) returns `None`.
    pub fn take_disconnects(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.disconnects)
    }

    /// Replies to connection `id`, or does nothing if it is gone -- the
    /// `Disconnected` event prunes it.
    pub fn reply(&self, id: u64, bytes: &[u8]) {
        if let Some(conn) = self.conns.get(&id) {
            conn.reply(id, bytes);
        }
    }
}

/// The sink behind a [`Hub`]: queue the event, then wake the loop.
fn queue_and_wake<C: Send + 'static>(tx: SyncSender<Event<C>>, waker: Waker) -> impl Sink<C> {
    move |event| {
        if tx.send(event).is_err() {
            return false; // the hub (and its loop) are gone
        }
        waker.wake();
        true
    }
}

/// Test helper: polls `hub` until a frame arrives (the handshake and the
/// cross-thread hop take a moment).
#[cfg(test)]
fn wait_frame<C: Conn>(hub: &mut Hub<C>) -> (u64, Vec<u8>) {
    for _ in 0..200 {
        if let Some(frame) = hub.next_frame() {
            return frame;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("no frame arrived within the deadline");
}
