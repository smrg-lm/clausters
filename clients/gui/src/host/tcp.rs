//! TCP server front for the `/gui_*` protocol (G25).
//!
//! The variant the [`super::ClientId`] seam anticipated: the audio server's
//! `osc::tcp` pattern reused — an acceptor thread plus one reader thread per
//! connection turn each byte stream into whole, length-prefixed OSC frames
//! (a 4-byte big-endian length prefix, the same framing scsynth and the audio
//! server use) — with one generalization: the readers hand events to a
//! caller-supplied **sink** instead of a fixed channel, because the host has
//! two fronts with different inboxes. The headless front wraps an
//! [`mpsc`](std::sync::mpsc) channel plus the zero-length-UDP wake (the
//! [`TcpHub`], mirroring the audio server); the windowed front wraps the winit
//! `EventLoopProxy`, which needs no wake at all.
//!
//! Frames above the configurable ceiling (`--max-frame`, default
//! [`clausters_core::osc::DEFAULT_MAX_FRAME`]) — or a zero prefix — close the
//! connection instead of allocating on an untrusted length.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

/// What a reader thread hands the front.
pub enum TcpEvent {
    /// A new connection: its id and the write half for replies.
    Connected(u64, TcpStream),
    /// A complete OSC frame from connection `id`.
    Frame(u64, Vec<u8>),
    /// Connection `id` closed (EOF or error).
    Disconnected(u64),
}

/// Binds a TCP listener on `addr` and starts accepting, handing every
/// [`TcpEvent`] to `sink` (called from the acceptor/reader threads; return
/// `false` when the consumer is gone to stop them). Returns the bound address.
pub fn bind_with_sink(
    addr: impl ToSocketAddrs,
    max_frame: usize,
    sink: impl Fn(TcpEvent) -> bool + Send + Clone + 'static,
) -> io::Result<SocketAddr> {
    let listener = TcpListener::bind(addr)?;
    let local_addr = listener.local_addr()?;
    std::thread::Builder::new()
        .name("clausters-gui-tcp-accept".into())
        .spawn(move || accept_loop(listener, max_frame, sink))
        .expect("failed to spawn the GUI TCP acceptor thread");
    Ok(local_addr)
}

/// The headless front's consumer: the event stream the serve loop drains and
/// the connection write halves it replies through — the audio server's
/// `TcpHub` shape. Reader threads wake the loop with a zero-length datagram to
/// `wake_target` (the host's own UDP address) after queuing an event.
pub struct TcpHub {
    events: Receiver<TcpEvent>,
    /// Write halves by connection id; writing goes through `&TcpStream`
    /// (a shared borrow), dead connections are pruned on `Disconnected`.
    conns: HashMap<u64, TcpStream>,
    local_addr: SocketAddr,
}

impl TcpHub {
    pub fn bind(
        addr: impl ToSocketAddrs,
        wake_target: SocketAddr,
        max_frame: usize,
    ) -> io::Result<Self> {
        let (tx, rx) = channel();
        // A throwaway UDP socket the reader threads use only to send wake bytes.
        let wake = UdpSocket::bind(("127.0.0.1", 0))?;
        let sink = MpscSink {
            tx,
            wake: std::sync::Arc::new(wake),
            wake_target,
        };
        let local_addr = bind_with_sink(addr, max_frame, move |event| sink.send(event))?;
        Ok(Self {
            events: rx,
            conns: HashMap::new(),
            local_addr,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// The next complete frame `(connection id, bytes)`, or `None` when the
    /// queue is drained. Registers and forgets connections as their
    /// `Connected`/`Disconnected` events go by — both bracket that
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
                }
                TcpEvent::Frame(id, bytes) => return Some((id, bytes)),
            }
        }
        None
    }

    /// Writes a length-prefixed reply to connection `id` (silently dropped if
    /// the connection is gone — the `Disconnected` event prunes it).
    pub fn reply(&self, id: u64, bytes: &[u8]) {
        if let Some(stream) = self.conns.get(&id)
            && let Err(e) = write_frame(stream, bytes)
        {
            tracing::warn!("failed to send reply to tcp client {id}: {e}");
        }
    }
}

/// The mpsc + wake sink behind [`TcpHub`].
#[derive(Clone)]
struct MpscSink {
    tx: Sender<TcpEvent>,
    wake: std::sync::Arc<UdpSocket>,
    wake_target: SocketAddr,
}

impl MpscSink {
    fn send(&self, event: TcpEvent) -> bool {
        if self.tx.send(event).is_err() {
            return false; // the hub (and front) are gone
        }
        let _ = self.wake.send_to(&[], self.wake_target);
        true
    }
}

fn accept_loop(
    listener: TcpListener,
    max_frame: usize,
    sink: impl Fn(TcpEvent) -> bool + Send + Clone + 'static,
) {
    let next_id = AtomicU64::new(1);
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let _ = stream.set_nodelay(true);
        let Ok(write_half) = stream.try_clone() else {
            continue;
        };
        let id = next_id.fetch_add(1, Ordering::Relaxed);
        // Register the connection before its reader can produce any frame.
        if !sink(TcpEvent::Connected(id, write_half)) {
            return; // the consumer is gone
        }
        let sink = sink.clone();
        std::thread::Builder::new()
            .name(format!("clausters-gui-tcp-{id}"))
            .spawn(move || reader_loop(id, stream, max_frame, sink))
            .ok();
    }
}

fn reader_loop(id: u64, mut stream: TcpStream, max_frame: usize, sink: impl Fn(TcpEvent) -> bool) {
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
        if !sink(TcpEvent::Frame(id, frame)) {
            return;
        }
    }
    let _ = sink(TcpEvent::Disconnected(id));
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
    use clausters_core::osc::{OscMessage, OscPacket, OscType, encode};

    /// A length-prefixed frame round-trips through the hub — in, decoded, and
    /// a reply routed back framed on the same connection. In-process, like the
    /// audio server's ws test.
    #[test]
    fn framed_packet_round_trips_through_the_hub() {
        let wake = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let wake_addr = wake.local_addr().unwrap();
        let mut hub = TcpHub::bind(
            ("127.0.0.1", 0),
            wake_addr,
            clausters_core::osc::DEFAULT_MAX_FRAME,
        )
        .unwrap();

        let mut client = TcpStream::connect(hub.local_addr()).unwrap();
        let msg = OscPacket::Message(OscMessage {
            addr: "/gui_query".into(),
            args: vec![OscType::Int(1)],
        });
        let bytes = encode(&msg).unwrap();
        write_frame(&client, &bytes).unwrap();

        // Drain until the frame arrives (Connected precedes it in the queue).
        let (id, got) = loop {
            if let Some(frame) = hub.next_frame() {
                break frame;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(got, bytes);

        // A reply routes back framed on the same connection.
        hub.reply(id, &bytes);
        let mut prefix = [0u8; 4];
        client.read_exact(&mut prefix).unwrap();
        let len = u32::from_be_bytes(prefix) as usize;
        let mut reply = vec![0u8; len];
        client.read_exact(&mut reply).unwrap();
        assert_eq!(reply, bytes);
    }
}
