//! WebSocket transport for the OSC server.
//!
//! One more **carrier of the same OSC encoding** beside UDP, TCP and the
//! shared-memory ring. The point is reach: a browser cannot open a raw UDP
//! socket or map shared memory, but it speaks WebSocket natively, so this is
//! the transport that lets the server run in (or be driven from) a browser —
//! and, conversely, lets a browser-hosted GUI peer reach the server.
//!
//! It mirrors [`super::tcp`] exactly: an acceptor thread plus one thread per
//! connection turn the socket into whole OSC packets handed to the
//! single-threaded command loop over an [`mpsc`](std::sync::mpsc) channel, and a
//! **zero-length UDP datagram** to the server's own address wakes the loop the
//! instant a frame (or a disconnect) is queued. The one structural difference
//! is the reply path: a [`tungstenite`] `WebSocket` owns its stream (read and
//! write are not split like a `TcpStream`), so instead of the loop owning a
//! write half, each connection thread also drains a per-connection reply channel
//! and writes the bytes itself. To interleave reads with those queued replies
//! the thread polls with a short read timeout (`POLL_TIMEOUT`) — the same
//! bounded-latency trade-off the IPC ring documents, here for the reply leg.
//!
//! Wire framing: each WebSocket **binary** message carries exactly one OSC
//! packet (a message or a bundle). WebSocket already frames messages, so —
//! unlike raw TCP — there is no length prefix; the frame boundary *is* the
//! packet boundary, and replies go back as binary messages the same way. Every
//! inbound packet decodes through the single [`super::decode_packet`] door, so
//! WebSocket bytes are validated exactly like UDP/TCP/ring bytes. `tungstenite`
//! enforces the maximum message size — the same configurable ceiling the TCP
//! transport applies to its length prefix (see [`super::DEFAULT_MAX_FRAME`]).

use std::collections::HashMap;
use std::io;
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::time::Duration;

use tungstenite::Message;

use super::ClientSlots;

/// How long a connection thread blocks on a read before it loops back to flush
/// queued replies (and any control-frame pong). Bounds reply latency when the
/// client is otherwise idle; small enough to feel immediate, large enough not
/// to spin. Same spirit as the ring's poll tick.
const POLL_TIMEOUT: Duration = Duration::from_millis(5);

/// Capacity, in packets, of the bounded channel from the connection threads to
/// the command loop. A flooding client blocks its own thread here and TCP flow
/// control pushes back to the sender — bounding server memory instead of
/// growing an unbounded queue (mirrors the TCP front's `INBOUND_QUEUE`).
const INBOUND_QUEUE: usize = 256;

/// Capacity, in replies, of each connection's outbound queue. The connection
/// thread drains it every poll tick; a backlog this deep means the client has
/// stopped reading (its socket is full), so the overflowing `reply` drops the
/// connection rather than queue without bound — the WebSocket counterpart of
/// the TCP front's reply write timeout.
const REPLY_QUEUE: usize = 256;

/// What a connection thread hands the command loop.
enum WsEvent {
    /// A new connection (handshake done): its id, the channel its replies go
    /// out through, and a raw handle to the socket so an overflowing `reply`
    /// can force-drop the connection.
    Connected(u64, SyncSender<Vec<u8>>, TcpStream),
    /// A complete OSC packet (one binary message) from connection `id`.
    Frame(u64, Vec<u8>),
    /// Connection `id` closed (clean close, EOF or error).
    Disconnected(u64),
}

/// The server side of the WebSocket transport: the event stream the loop drains
/// and the per-connection reply channels it answers through. The `TcpListener`
/// lives in the acceptor thread; dropping the hub drops the channel, which makes
/// the connection threads exit on their next send.
pub struct WsHub {
    events: Receiver<WsEvent>,
    /// Reply channels by connection id (plus a raw socket handle to force-drop
    /// a slow consumer). A reply is queued here and the owning connection
    /// thread writes it as a binary frame; dead connections are pruned on
    /// `Disconnected`.
    conns: HashMap<u64, (SyncSender<Vec<u8>>, TcpStream)>,
    /// Connection ids whose `Disconnected` went by since the last
    /// [`take_disconnects`](Self::take_disconnects), so the command loop can
    /// drop per-client state (bus streams, `/notify` registrations).
    disconnects: Vec<u64>,
    local_addr: SocketAddr,
}

impl WsHub {
    /// Binds a TCP listener on `addr` and starts accepting WebSocket upgrades.
    /// `wake_target` is the server's own UDP address; connection threads send a
    /// zero-length datagram there to wake the command loop when a frame or
    /// disconnect is queued. `max_frame` is the largest message accepted on a
    /// connection (see [`super::DEFAULT_MAX_FRAME`]), enforced by tungstenite.
    /// `slots` is the live-client ceiling (`--max-clients`), shared with the
    /// TCP front; a connection past it is dropped at accept.
    pub fn bind(
        addr: impl ToSocketAddrs,
        wake_target: SocketAddr,
        max_frame: usize,
        slots: Arc<ClientSlots>,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        let local_addr = listener.local_addr()?;
        let (tx, rx) = sync_channel(INBOUND_QUEUE);
        // A throwaway UDP socket the connection threads use only for wake bytes.
        let wake = Arc::new(UdpSocket::bind(("127.0.0.1", 0))?);
        std::thread::Builder::new()
            .name("clausters-ws-accept".into())
            .spawn(move || accept_loop(listener, tx, wake, wake_target, max_frame, slots))
            .expect("failed to spawn the WebSocket acceptor thread");
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

    /// The next complete packet `(connection id, bytes)`, or `None` when the
    /// queue is drained. Registers and forgets connections as their
    /// `Connected`/`Disconnected` events go by — both bracket that connection's
    /// frames in the channel, so the reply channel is always present before a
    /// frame is returned for handling.
    pub fn next_frame(&mut self) -> Option<(u64, Vec<u8>)> {
        while let Ok(event) = self.events.try_recv() {
            match event {
                WsEvent::Connected(id, reply_tx, raw) => {
                    self.conns.insert(id, (reply_tx, raw));
                }
                WsEvent::Disconnected(id) => {
                    self.conns.remove(&id);
                    self.disconnects.push(id);
                }
                WsEvent::Frame(id, bytes) => return Some((id, bytes)),
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

    /// Queues a reply to connection `id` (silently dropped if the connection is
    /// gone — the `Disconnected` event prunes it). The owning connection thread
    /// writes it as a binary frame on its next poll. A full queue means the
    /// client has stopped reading, so the connection is dropped rather than
    /// queueing without bound; its thread sees the shutdown and the
    /// `Disconnected` event prunes the state.
    pub fn reply(&self, id: u64, bytes: &[u8]) {
        let Some((reply_tx, raw)) = self.conns.get(&id) else {
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
}

fn accept_loop(
    listener: TcpListener,
    tx: SyncSender<WsEvent>,
    wake: Arc<UdpSocket>,
    wake_target: SocketAddr,
    max_frame: usize,
    slots: Arc<ClientSlots>,
) {
    let next_id = AtomicU64::new(1);
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let Some(slot) = slots.try_acquire() else {
            tracing::warn!("refusing ws connection: --max-clients ceiling reached");
            continue; // dropping the stream closes it
        };
        let _ = stream.set_nodelay(true);
        let id = next_id.fetch_add(1, Ordering::Relaxed);
        let tx = tx.clone();
        let wake = Arc::clone(&wake);
        // The handshake runs in the per-connection thread (not here), so a slow
        // or non-WebSocket peer cannot stall the acceptor.
        std::thread::Builder::new()
            .name(format!("clausters-ws-{id}"))
            .spawn(move || {
                // The slot rides the connection thread and frees on any exit.
                let _slot = slot;
                conn_loop(id, stream, tx, &wake, wake_target, max_frame)
            })
            .ok();
    }
}

fn conn_loop(
    id: u64,
    stream: TcpStream,
    tx: SyncSender<WsEvent>,
    wake: &UdpSocket,
    wake_target: SocketAddr,
    max_frame: usize,
) {
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
    // The raw handle lets an overflowing `reply` force-drop the connection.
    let Ok(raw) = ws.get_ref().try_clone() else {
        return;
    };
    let (reply_tx, reply_rx) = sync_channel::<Vec<u8>>(REPLY_QUEUE);
    if tx.send(WsEvent::Connected(id, reply_tx, raw)).is_err() {
        return; // the loop (and hub) are gone
    }

    loop {
        match ws.read() {
            Ok(Message::Binary(bytes)) => {
                if tx.send(WsEvent::Frame(id, bytes)).is_err() {
                    return;
                }
                let _ = wake.send_to(&[], wake_target);
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

    let _ = tx.send(WsEvent::Disconnected(id));
    let _ = wake.send_to(&[], wake_target);
}

#[cfg(test)]
mod tests {
    use super::*;
    use rosc::{OscMessage, OscPacket, OscType, encoder};

    /// An OSC packet sent as one WebSocket binary message arrives intact and
    /// decodes through the single door, and a reply routes back to the same
    /// connection as a binary message. Runs entirely in-process (client and hub
    /// in one test), so localhost sockets are reachable.
    #[test]
    fn binary_message_round_trips_one_osc_packet() {
        // A throwaway UDP socket plays the role of the command loop's wake
        // target; we only need it to exist so the hub's wake sends succeed.
        let wake = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let wake_addr = wake.local_addr().unwrap();

        let mut hub = WsHub::bind(
            ("127.0.0.1", 0),
            wake_addr,
            crate::osc::DEFAULT_MAX_FRAME,
            Arc::new(ClientSlots::new(crate::osc::DEFAULT_MAX_CLIENTS)),
        )
        .unwrap();
        let port = hub.local_addr().port();

        // Client side: a tungstenite WebSocket over a plain TCP connection.
        let (mut client, _resp) =
            tungstenite::connect(format!("ws://127.0.0.1:{port}/")).expect("ws connect");

        // Send one OSC message as a single binary frame.
        let msg = OscPacket::Message(OscMessage {
            addr: "/status".into(),
            args: vec![],
        });
        let bytes = encoder::encode(&msg).unwrap();
        client.send(Message::Binary(bytes.clone())).unwrap();

        // The hub yields exactly those bytes, and they decode.
        let (conn_id, got) = wait_for_frame(&mut hub);
        assert_eq!(got, bytes, "the binary frame is one whole OSC packet");
        assert!(crate::osc::decode_packet(&got).is_ok());

        // A reply routes back to the same connection as a binary message.
        let reply = OscPacket::Message(OscMessage {
            addr: "/status.reply".into(),
            args: vec![OscType::Int(7)],
        });
        let reply_bytes = encoder::encode(&reply).unwrap();
        hub.reply(conn_id, &reply_bytes);

        match read_binary(&mut client) {
            Message::Binary(b) => assert_eq!(b, reply_bytes),
            other => panic!("expected a binary reply, got {other:?}"),
        }
    }

    /// Polls the hub until its connection thread has delivered the frame (the
    /// handshake and the cross-thread hop take a moment).
    fn wait_for_frame(hub: &mut WsHub) -> (u64, Vec<u8>) {
        for _ in 0..200 {
            if let Some(frame) = hub.next_frame() {
                return frame;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("no frame arrived within the deadline");
    }

    /// Reads from the client until a non-control message arrives. Generic over
    /// the stream type, since `tungstenite::connect` yields a `MaybeTlsStream`.
    fn read_binary<S: io::Read + io::Write>(client: &mut tungstenite::WebSocket<S>) -> Message {
        for _ in 0..200 {
            match client.read() {
                Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => continue,
                Ok(msg) => return msg,
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
}
