//! OSC over TCP.
//!
//! Wire framing (the same as scsynth's TCP): a 4-byte big-endian length prefix
//! followed by exactly that many OSC bytes -- a message or a bundle. Replies
//! use the same framing. A zero prefix, or one above `max_frame`, closes the
//! connection instead of allocating on an untrusted length.
//!
//! The reply path is the connection's write half, a clone of the accepted
//! stream, written by whoever answers -- the command loop itself. A client
//! that stops reading would fill its socket buffer and stall that loop, so
//! every accepted stream carries [`REPLY_WRITE_TIMEOUT`], and a write that
//! fails or times out drops the connection.

use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::{ClientSlots, Conn, Event, Hub, Sink, Waker, queue_and_wake};

/// How long a reply write may block before the connection is dropped. Generous
/// -- it fires only when the client makes no progress at all for this long.
pub const REPLY_WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// The reply half of a TCP connection: its write half.
#[derive(Debug)]
pub struct TcpConn(TcpStream);

impl Conn for TcpConn {
    fn reply(&self, id: u64, bytes: &[u8]) {
        if let Err(e) = write_frame(&self.0, bytes) {
            tracing::warn!("dropping tcp client {id}: reply failed: {e}");
            let _ = self.0.shutdown(Shutdown::Both);
        }
    }
}

/// The hub of a TCP front.
pub type TcpHub = Hub<TcpConn>;

/// Binds `addr` and starts a [`TcpHub`] on it: every event is queued and
/// `waker` is woken after it.
pub fn hub(
    addr: impl ToSocketAddrs,
    waker: Waker,
    max_frame: usize,
    slots: Arc<ClientSlots>,
) -> io::Result<TcpHub> {
    Hub::serve(waker, |tx, waker| {
        bind(addr, max_frame, slots, queue_and_wake(tx, waker))
    })
}

/// Binds `addr` and starts accepting, handing every [`Event`] to `sink`.
/// Returns the bound address.
pub fn bind(
    addr: impl ToSocketAddrs,
    max_frame: usize,
    slots: Arc<ClientSlots>,
    sink: impl Sink<TcpConn>,
) -> io::Result<SocketAddr> {
    let listener = TcpListener::bind(addr)?;
    let local_addr = listener.local_addr()?;
    std::thread::Builder::new()
        .name("clausters-tcp-accept".into())
        .spawn(move || accept_loop(listener, max_frame, slots, sink))?;
    Ok(local_addr)
}

fn accept_loop(
    listener: TcpListener,
    max_frame: usize,
    slots: Arc<ClientSlots>,
    sink: impl Sink<TcpConn>,
) {
    let next_id = AtomicU64::new(1);
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let Some(slot) = slots.try_acquire() else {
            tracing::warn!("refusing tcp connection: the client ceiling is reached");
            continue; // dropping the stream closes it
        };
        let _ = stream.set_nodelay(true);
        // The write half shares the socket, so the timeout set here bounds
        // every reply write.
        let _ = stream.set_write_timeout(Some(REPLY_WRITE_TIMEOUT));
        let Ok(write_half) = stream.try_clone() else {
            continue;
        };
        let id = next_id.fetch_add(1, Ordering::Relaxed);
        // Register the connection before its reader can produce any frame.
        if !sink(Event::Connected(id, TcpConn(write_half))) {
            return; // the consumer is gone
        }
        let sink = sink.clone();
        std::thread::Builder::new()
            .name(format!("clausters-tcp-{id}"))
            .spawn(move || {
                // The slot rides the reader thread and frees on any exit.
                let _slot = slot;
                reader_loop(id, stream, max_frame, sink)
            })
            .ok();
    }
}

fn reader_loop(id: u64, mut stream: TcpStream, max_frame: usize, sink: impl Sink<TcpConn>) {
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
        if !sink(Event::Frame(id, frame)) {
            return;
        }
    }
    // The write half is a clone of this socket and lives on until the consumer
    // reads the disconnect, so dropping this end alone would not close it: the
    // client would wait on a connection nobody serves.
    let _ = stream.shutdown(Shutdown::Both);
    let _ = sink(Event::Disconnected(id));
}

/// Writes `bytes` framed with a 4-byte big-endian length prefix. `&TcpStream`
/// implements [`Write`], so this takes a shared borrow.
pub fn write_frame(mut w: impl Write, bytes: &[u8]) -> io::Result<()> {
    let len = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "OSC frame too large for TCP"))?;
    w.write_all(&len.to_be_bytes())?;
    w.write_all(bytes)?;
    w.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wait_frame;
    use std::net::UdpSocket;

    const MAX_FRAME: usize = 1 << 20;

    fn test_hub(slots: usize) -> (TcpHub, UdpSocket) {
        // The socket plays the command loop's; it only has to exist.
        let wake = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let waker = Waker::to(wake.local_addr().unwrap()).unwrap();
        let hub = hub(
            ("127.0.0.1", 0),
            waker,
            MAX_FRAME,
            Arc::new(ClientSlots::new(slots)),
        )
        .unwrap();
        (hub, wake)
    }

    fn read_frame(stream: &mut TcpStream) -> Vec<u8> {
        let mut prefix = [0u8; 4];
        stream.read_exact(&mut prefix).unwrap();
        let mut frame = vec![0u8; u32::from_be_bytes(prefix) as usize];
        stream.read_exact(&mut frame).unwrap();
        frame
    }

    /// A length-prefixed frame arrives whole, a reply goes back framed on the
    /// same connection, and closing it is reported. In-process, so localhost
    /// sockets are reachable.
    #[test]
    fn a_frame_round_trips_through_the_hub() {
        let (mut hub, _wake) = test_hub(4);
        let mut client = TcpStream::connect(hub.local_addr()).unwrap();
        write_frame(&client, b"/png").unwrap();
        let (id, got) = wait_frame(&mut hub);
        assert_eq!(got, b"/png");

        hub.reply(id, b"/pong");
        assert_eq!(read_frame(&mut client), b"/pong");

        drop(client);
        for _ in 0..200 {
            hub.next_frame();
            if hub.take_disconnects() == [id] {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the disconnect was never reported");
    }

    /// A prefix above the ceiling closes the connection without a frame.
    #[test]
    fn a_frame_past_the_ceiling_closes_the_connection() {
        let (mut hub, _wake) = test_hub(4);
        let mut client = TcpStream::connect(hub.local_addr()).unwrap();
        client
            .write_all(&(MAX_FRAME as u32 + 1).to_be_bytes())
            .unwrap();
        let mut buf = [0u8; 1];
        assert!(matches!(client.read(&mut buf), Ok(0) | Err(_)));
        assert!(hub.next_frame().is_none());
    }

    /// The client ceiling: with one slot, the first connection is served, the
    /// second is dropped at accept, and closing the first frees its slot for a
    /// third.
    #[test]
    fn the_client_ceiling_drops_and_recycles() {
        let (mut hub, _wake) = test_hub(1);
        let addr = hub.local_addr();
        let is_closed = |stream: &mut TcpStream| {
            // The acceptor never announces the refused stream; the drop
            // surfaces to the client as EOF (or a reset) on its next read.
            let mut buf = [0u8; 1];
            matches!(stream.read(&mut buf), Ok(0) | Err(_))
        };

        let first = TcpStream::connect(addr).unwrap();
        write_frame(&first, b"/png").unwrap();
        let (first_id, bytes) = wait_frame(&mut hub);
        assert_eq!(bytes, b"/png");

        // The write may already fail with a reset, which is the drop showing.
        let mut second = TcpStream::connect(addr).unwrap();
        let _ = write_frame(&second, b"/png");
        assert!(
            is_closed(&mut second),
            "the second connection must be dropped"
        );

        // The slot releases when the first reader thread exits, a moment
        // after the disconnect, so the third connection retries.
        drop(first);
        for _ in 0..100 {
            let third = TcpStream::connect(addr).unwrap();
            let _ = write_frame(&third, b"/png");
            for _ in 0..10 {
                if let Some((id, bytes)) = hub.next_frame() {
                    assert_eq!(bytes, b"/png");
                    assert_ne!(id, first_id);
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        panic!("a freed slot must serve a new connection");
    }
}
