//! WebSocket server front for the `/gui_*` protocol.
//!
//! The browser's carrier into a **native** host: a page cannot open a raw UDP
//! socket or a TCP connection, but it speaks WebSocket natively — this is the
//! leg that lets the TypeScript client drive a desktop `clausters-gui` the way
//! it drives a `clausters --ws` audio server. It mirrors the audio server's
//! `osc::ws` exactly, with the same generalization [`super::tcp`] has: the
//! per-connection threads hand events to a caller-supplied **sink**, because
//! the host has two fronts with different inboxes (the headless serve loop's
//! mpsc + zero-length-UDP wake, wrapped by [`WsHub`]; the windowed front's
//! winit `EventLoopProxy`, which needs no wake at all).
//!
//! Wire framing: each WebSocket **binary** message carries exactly one OSC
//! packet — WebSocket already frames messages, so unlike raw TCP there is no
//! length prefix; the frame boundary *is* the packet boundary, and replies go
//! back as binary messages the same way. `tungstenite` enforces the maximum
//! message size (the `--max-frame` ceiling the TCP leg applies to its length
//! prefix). One structural difference from TCP: a `tungstenite` `WebSocket`
//! owns its stream (read and write are not split), so instead of the front
//! holding a write half, each connection thread drains a per-connection reply
//! channel and writes the bytes itself, polling with a short read timeout
//! ([`POLL_TIMEOUT`]) to interleave reads with queued replies.

use std::collections::HashMap;
use std::io;
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, TrySendError, channel, sync_channel};
use std::time::Duration;

use tungstenite::Message;

/// How long a connection thread blocks on a read before it loops back to flush
/// queued replies (and any control-frame pong). Bounds reply latency when the
/// client is otherwise idle; the audio server's `osc::ws` value.
const POLL_TIMEOUT: Duration = Duration::from_millis(5);

/// Capacity, in replies, of each connection's outbound queue. A backlog this
/// deep means the client has stopped reading, so the overflowing reply drops
/// the connection rather than queue without bound.
const REPLY_QUEUE: usize = 256;

/// What a connection thread hands the front.
pub enum WsEvent {
    /// A new connection (handshake done): its id, the channel its replies go
    /// out through, and a raw handle to the socket so an overflowing reply can
    /// force-drop the connection.
    Connected(u64, SyncSender<Vec<u8>>, TcpStream),
    /// A complete OSC packet (one binary message) from connection `id`.
    Frame(u64, Vec<u8>),
    /// Connection `id` closed (clean close, EOF or error).
    Disconnected(u64),
}

/// Binds a TCP listener on `addr` and starts accepting WebSocket upgrades,
/// handing every [`WsEvent`] to `sink` (called from the connection threads;
/// return `false` when the consumer is gone to stop them). Returns the bound
/// address. Binds like the audio server's `--ws`: reachable from a browser on
/// another machine too, not loopback-only.
pub fn bind_with_sink(
    addr: impl ToSocketAddrs,
    max_frame: usize,
    sink: impl Fn(WsEvent) -> bool + Send + Clone + 'static,
) -> io::Result<SocketAddr> {
    let listener = TcpListener::bind(addr)?;
    let local_addr = listener.local_addr()?;
    std::thread::Builder::new()
        .name("clausters-gui-ws-accept".into())
        .spawn(move || accept_loop(listener, max_frame, sink))
        .expect("failed to spawn the GUI WebSocket acceptor thread");
    Ok(local_addr)
}

/// Routes one queued reply to connection `id` through its channel — the shared
/// routing both fronts use (the headless hub and the windowed app each hold a
/// `conns` map of `(reply channel, raw socket)`). A full queue means the
/// client has stopped reading: the connection is force-dropped rather than
/// queueing without bound, and its `Disconnected` event prunes the map.
pub fn reply(conns: &HashMap<u64, (SyncSender<Vec<u8>>, TcpStream)>, id: u64, bytes: &[u8]) {
    let Some((reply_tx, raw)) = conns.get(&id) else {
        return;
    };
    match reply_tx.try_send(bytes.to_vec()) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            tracing::warn!("dropping ws client {id}: not draining its replies");
            let _ = raw.shutdown(Shutdown::Both);
        }
        Err(TrySendError::Disconnected(_)) => {
            tracing::warn!("ws client {id} reply channel closed before send");
        }
    }
}

/// The headless front's consumer: the event stream the serve loop drains and
/// the per-connection reply channels it answers through — the [`super::tcp::TcpHub`]
/// shape. Connection threads wake the loop with a zero-length datagram to
/// `wake_target` (the host's own UDP address) after queuing an event.
pub struct WsHub {
    events: Receiver<WsEvent>,
    /// Reply channels by connection id (plus the raw socket handle `reply`
    /// uses to force-drop a slow consumer); pruned on `Disconnected`.
    conns: HashMap<u64, (SyncSender<Vec<u8>>, TcpStream)>,
    local_addr: SocketAddr,
}

impl WsHub {
    pub fn bind(
        addr: impl ToSocketAddrs,
        wake_target: SocketAddr,
        max_frame: usize,
    ) -> io::Result<Self> {
        let (tx, rx) = channel();
        // A throwaway UDP socket the connection threads use only for wake bytes.
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

    /// The next complete packet `(connection id, bytes)`, or `None` when the
    /// queue is drained. Registers and forgets connections as their
    /// `Connected`/`Disconnected` events go by — both bracket that
    /// connection's frames in the channel, so the reply channel is always
    /// present before a frame is returned for handling.
    pub fn next_frame(&mut self) -> Option<(u64, Vec<u8>)> {
        while let Ok(event) = self.events.try_recv() {
            match event {
                WsEvent::Connected(id, reply_tx, raw) => {
                    self.conns.insert(id, (reply_tx, raw));
                }
                WsEvent::Disconnected(id) => {
                    self.conns.remove(&id);
                }
                WsEvent::Frame(id, bytes) => return Some((id, bytes)),
            }
        }
        None
    }

    /// Queues a reply to connection `id` (silently dropped if the connection
    /// is gone — the `Disconnected` event prunes it).
    pub fn reply(&self, id: u64, bytes: &[u8]) {
        reply(&self.conns, id, bytes);
    }
}

/// The mpsc + wake sink behind [`WsHub`].
#[derive(Clone)]
struct MpscSink {
    tx: Sender<WsEvent>,
    wake: std::sync::Arc<UdpSocket>,
    wake_target: SocketAddr,
}

impl MpscSink {
    fn send(&self, event: WsEvent) -> bool {
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
    sink: impl Fn(WsEvent) -> bool + Send + Clone + 'static,
) {
    let next_id = AtomicU64::new(1);
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let _ = stream.set_nodelay(true);
        let id = next_id.fetch_add(1, Ordering::Relaxed);
        let sink = sink.clone();
        // The handshake runs in the per-connection thread (not here), so a
        // slow or non-WebSocket peer cannot stall the acceptor.
        std::thread::Builder::new()
            .name(format!("clausters-gui-ws-{id}"))
            .spawn(move || conn_loop(id, stream, max_frame, sink))
            .ok();
    }
}

fn conn_loop(id: u64, stream: TcpStream, max_frame: usize, sink: impl Fn(WsEvent) -> bool) {
    // The handshake completes on the still-blocking stream; a non-WebSocket
    // peer just fails it and the connection is dropped, never announced.
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
    // The raw handle lets an overflowing `reply` force-drop the connection.
    let Ok(raw) = ws.get_ref().try_clone() else {
        return;
    };
    let (reply_tx, reply_rx) = sync_channel::<Vec<u8>>(REPLY_QUEUE);
    if !sink(WsEvent::Connected(id, reply_tx, raw)) {
        return; // the front (and its inbox) are gone
    }

    loop {
        match ws.read() {
            Ok(Message::Binary(bytes)) => {
                if !sink(WsEvent::Frame(id, bytes)) {
                    return;
                }
            }
            Ok(Message::Close(_)) => break,
            // Text/Ping/Pong/raw frames: nothing to route. tungstenite answers
            // a ping by queueing the pong itself, flushed below.
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
        while let Ok(bytes) = reply_rx.try_recv() {
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

    let _ = sink(WsEvent::Disconnected(id));
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_core::osc::{OscMessage, OscPacket, OscType, encode};

    /// An OSC packet sent as one WebSocket binary message round-trips through
    /// the hub — in, decoded, and a reply routed back as a binary message on
    /// the same connection. In-process, like the tcp front's test and the
    /// audio server's ws test.
    #[test]
    fn binary_message_round_trips_through_the_hub() {
        let wake = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let wake_addr = wake.local_addr().unwrap();
        let mut hub = WsHub::bind(
            ("127.0.0.1", 0),
            wake_addr,
            clausters_core::osc::DEFAULT_MAX_FRAME,
        )
        .unwrap();
        let port = hub.local_addr().port();

        let (mut client, _resp) =
            tungstenite::connect(format!("ws://127.0.0.1:{port}/")).expect("ws connect");

        let msg = OscPacket::Message(OscMessage {
            addr: "/gui_query".into(),
            args: vec![OscType::Int(1)],
        });
        let bytes = encode(&msg).unwrap();
        client.send(Message::Binary(bytes.clone())).unwrap();

        // Drain until the frame arrives (Connected precedes it in the queue).
        let (id, got) = loop {
            if let Some(frame) = hub.next_frame() {
                break frame;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(got, bytes, "the binary frame is one whole OSC packet");
        assert!(clausters_core::osc::decode_packet(&got).is_ok());

        // A reply routes back to the same connection as a binary message.
        hub.reply(id, &bytes);
        loop {
            match client.read() {
                Ok(Message::Binary(b)) => {
                    assert_eq!(b, bytes);
                    break;
                }
                Ok(_) => continue,
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
    }
}
