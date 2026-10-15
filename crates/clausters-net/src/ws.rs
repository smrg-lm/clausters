//! OSC over WebSocket: the carrier a browser can open.
//!
//! Wire framing: each WebSocket **binary** message carries exactly one OSC
//! packet (a message or a bundle). WebSocket already frames messages, so --
//! unlike raw TCP -- there is no length prefix; the frame boundary *is* the
//! packet boundary, and replies go back as binary messages the same way.
//! `tungstenite` enforces `max_frame` as the largest message and frame.
//!
//! The one structural difference from [`crate::tcp`] is the reply path: a
//! `tungstenite` `WebSocket` owns its stream (read and write are not split like
//! a `TcpStream`), so instead of the answering loop holding a write half, each
//! connection thread also drains a per-connection reply queue and writes the
//! bytes itself. To interleave reads with those queued replies the thread polls
//! with a short read timeout ([`POLL_TIMEOUT`]). The queue holds at most
//! [`REPLY_QUEUE_BYTES`] of replies; a reply that would overflow it drops the
//! connection -- the counterpart of the TCP carrier's write timeout. It is
//! bounded in bytes, not replies, because one answer can legitimately be many
//! replies at once (a plugin scan's catalogue is hundreds of fragments, sent
//! in one turn, faster than a 5 ms poll drains them).

use std::io;
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::time::Duration;

use tungstenite::Message;

use crate::{ClientSlots, Conn, Event, Hub, Sink, Waker, queue_and_wake};

/// How long a connection thread blocks on a read before it loops back to flush
/// queued replies (and any control-frame pong). Bounds reply latency when the
/// client is otherwise idle; small enough to feel immediate, large enough not
/// to spin.
pub const POLL_TIMEOUT: Duration = Duration::from_millis(5);

/// Capacity, in bytes, of each connection's outbound queue: twice the largest
/// single answer the server sends (a 16 MiB plugin catalogue). The connection
/// thread drains it every poll tick, so a backlog this deep means the client
/// has stopped reading.
pub const REPLY_QUEUE_BYTES: usize = 32 * 1024 * 1024;

/// The reply half of a WebSocket connection: the queue its thread drains, and
/// a raw handle to the socket so a reply that finds the queue full can drop the
/// connection.
#[derive(Debug)]
pub struct WsConn {
    replies: Sender<Vec<u8>>,
    /// Bytes queued and not yet written, shared with the connection thread.
    queued: Arc<AtomicUsize>,
    /// Set once the connection is dropped or gone: later replies are
    /// discarded without a word, so one lost client is one warning.
    dead: AtomicBool,
    raw: TcpStream,
}

impl Conn for WsConn {
    fn reply(&self, id: u64, bytes: &[u8]) {
        if self.dead.load(Ordering::Relaxed) {
            return;
        }
        if self.queued.load(Ordering::Acquire) + bytes.len() > REPLY_QUEUE_BYTES {
            self.dead.store(true, Ordering::Relaxed);
            tracing::warn!("dropping ws client {id}: not draining its replies");
            let _ = self.raw.shutdown(Shutdown::Both);
            return;
        }
        self.queued.fetch_add(bytes.len(), Ordering::AcqRel);
        if self.replies.send(bytes.to_vec()).is_err() {
            self.dead.store(true, Ordering::Relaxed);
            tracing::warn!("ws client {id} closed before its replies were sent");
        }
    }
}

/// The hub of a WebSocket front.
pub type WsHub = Hub<WsConn>;

/// Binds `addr` and starts a [`WsHub`] on it: every event is queued and `waker`
/// is woken after it.
pub fn hub(
    addr: impl ToSocketAddrs,
    waker: Waker,
    max_frame: usize,
    slots: Arc<ClientSlots>,
) -> io::Result<WsHub> {
    Hub::serve(waker, |tx, waker| {
        bind(addr, max_frame, slots, queue_and_wake(tx, waker))
    })
}

/// Binds `addr` and starts accepting WebSocket upgrades, handing every
/// [`Event`] to `sink`. Returns the bound address.
pub fn bind(
    addr: impl ToSocketAddrs,
    max_frame: usize,
    slots: Arc<ClientSlots>,
    sink: impl Sink<WsConn>,
) -> io::Result<SocketAddr> {
    let listener = TcpListener::bind(addr)?;
    let local_addr = listener.local_addr()?;
    std::thread::Builder::new()
        .name("clausters-ws-accept".into())
        .spawn(move || accept_loop(listener, max_frame, slots, sink))?;
    Ok(local_addr)
}

fn accept_loop(
    listener: TcpListener,
    max_frame: usize,
    slots: Arc<ClientSlots>,
    sink: impl Sink<WsConn>,
) {
    let next_id = AtomicU64::new(1);
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let Some(slot) = slots.try_acquire() else {
            tracing::warn!("refusing ws connection: the client ceiling is reached");
            continue; // dropping the stream closes it
        };
        let _ = stream.set_nodelay(true);
        let id = next_id.fetch_add(1, Ordering::Relaxed);
        let sink = sink.clone();
        // The handshake runs in the per-connection thread (not here), so a slow
        // or non-WebSocket peer cannot stall the acceptor.
        std::thread::Builder::new()
            .name(format!("clausters-ws-{id}"))
            .spawn(move || {
                // The slot rides the connection thread and frees on any exit.
                let _slot = slot;
                conn_loop(id, stream, max_frame, sink)
            })
            .ok();
    }
}

fn conn_loop(id: u64, stream: TcpStream, max_frame: usize, sink: impl Sink<WsConn>) {
    // The handshake completes on the still-blocking stream; a non-WebSocket peer
    // just fails it and the connection is dropped, never announced.
    let config = tungstenite::protocol::WebSocketConfig {
        max_message_size: Some(max_frame),
        max_frame_size: Some(max_frame),
        ..Default::default()
    };
    let mut ws = match tungstenite::accept_with_config(stream, Some(config)) {
        Ok(ws) => ws,
        Err(_) => return,
    };
    // Poll from here on so reads interleave with queued replies.
    let _ = ws.get_ref().set_read_timeout(Some(POLL_TIMEOUT));
    let Ok(raw) = ws.get_ref().try_clone() else {
        return;
    };
    let (replies, pending) = channel::<Vec<u8>>();
    let queued = Arc::new(AtomicUsize::new(0));
    let conn = WsConn {
        replies,
        queued: Arc::clone(&queued),
        dead: AtomicBool::new(false),
        raw,
    };
    if !sink(Event::Connected(id, conn)) {
        return; // the consumer is gone
    }

    loop {
        match ws.read() {
            Ok(Message::Binary(bytes)) => {
                if !sink(Event::Frame(id, bytes)) {
                    return;
                }
            }
            Ok(Message::Close(_)) => break,
            // Text/Ping/Pong/raw frames: nothing to route. tungstenite answers a
            // ping by queueing the pong itself, flushed below.
            Ok(_) => {}
            // A read timeout (no data within POLL_TIMEOUT) surfaces as a
            // would-block/timed-out I/O error: idle, not a failure.
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break, // protocol/connection error: drop it
        }
        // Flush every queued reply (and any pong tungstenite queued on a ping).
        let mut dead = false;
        while let Ok(bytes) = pending.try_recv() {
            queued.fetch_sub(bytes.len(), Ordering::AcqRel);
            if ws.send(Message::Binary(bytes)).is_err() {
                dead = true;
                break;
            }
        }
        let _ = ws.flush();
        if dead {
            break;
        }
    }

    let _ = sink(Event::Disconnected(id));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wait_frame;
    use std::net::UdpSocket;

    /// A packet sent as one binary message arrives intact, and a reply routes
    /// back to the same connection as a binary message. In-process, so
    /// localhost sockets are reachable.
    #[test]
    fn a_binary_message_round_trips_through_the_hub() {
        // The socket plays the command loop's; it only has to exist.
        let wake = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let waker = Waker::to(wake.local_addr().unwrap()).unwrap();
        let mut hub = hub(
            ("127.0.0.1", 0),
            waker,
            1 << 20,
            Arc::new(ClientSlots::new(4)),
        )
        .unwrap();
        let port = hub.local_addr().port();
        let (mut client, _resp) =
            tungstenite::connect(format!("ws://127.0.0.1:{port}/")).expect("ws connect");

        client.send(Message::Binary(b"/png".to_vec())).unwrap();
        let (id, got) = wait_frame(&mut hub);
        assert_eq!(got, b"/png", "the binary message is one whole packet");

        hub.reply(id, b"/pong");
        for _ in 0..200 {
            match client.read() {
                Ok(Message::Binary(b)) => return assert_eq!(b, b"/pong"),
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("client read failed: {e}"),
            }
        }
        panic!("no reply arrived within the deadline");
    }

    /// One answer can be hundreds of replies in one turn -- a plugin scan's
    /// catalogue -- far more than a poll tick drains. They all arrive, in
    /// order, and the client stays connected.
    #[test]
    fn a_burst_of_replies_in_one_turn_is_delivered_whole() {
        let wake = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let waker = Waker::to(wake.local_addr().unwrap()).unwrap();
        let mut hub = hub(
            ("127.0.0.1", 0),
            waker,
            1 << 20,
            Arc::new(ClientSlots::new(4)),
        )
        .unwrap();
        let port = hub.local_addr().port();
        let (mut client, _resp) =
            tungstenite::connect(format!("ws://127.0.0.1:{port}/")).expect("ws connect");
        client.send(Message::Binary(b"/scan".to_vec())).unwrap();
        let (id, _) = wait_frame(&mut hub);
        const REPLIES: usize = 1000;
        for index in 0..REPLIES {
            let mut reply = vec![0u8; 6144];
            reply[..8].copy_from_slice(&(index as u64).to_le_bytes());
            hub.reply(id, &reply);
        }
        let mut next = 0;
        while next < REPLIES {
            match client.read() {
                Ok(Message::Binary(b)) => {
                    assert_eq!(u64::from_le_bytes(b[..8].try_into().unwrap()), next as u64);
                    next += 1;
                }
                Ok(_) => {}
                Err(e) => panic!("reply {next} of {REPLIES} lost: {e}"),
            }
        }
    }
}
