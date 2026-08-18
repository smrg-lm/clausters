//! Waking the command loop from a thread that is not it.
//!
//! [`OscServer::run`](super::server::OscServer::run) blocks in `recv_from`
//! under a read timeout, so anything produced *off* the network thread — a TCP
//! frame, a MIDI message, a finished NRT job, a compiled Faust def — is only
//! seen when that recv returns. A **zero-length UDP datagram** to the server's
//! own address ends the blocking recv immediately: the loop's own idle tick
//! stays a housekeeping interval instead of doubling as the latency of every
//! result.
//!
//! The transports build the datagram by hand (they carry only the target
//! address across a thread boundary); a [`Waker`] is the same trick packaged
//! for the producers that live inside the server process — it owns the
//! throwaway socket it sends from, so a worker thread needs nothing but a
//! clone of it.

use std::net::{SocketAddr, UdpSocket};

/// A handle a worker thread pokes to end the command loop's blocking recv.
///
/// Cheap to clone (one socket shared by every clone), and silent on failure:
/// a wake that does not arrive costs one idle tick of latency, never a
/// result, so there is nothing useful to report from a worker thread.
#[derive(Clone)]
pub struct Waker {
    socket: std::sync::Arc<UdpSocket>,
    target: SocketAddr,
}

impl Waker {
    /// Opens a throwaway sender aimed at `target`, which is the address the
    /// command loop's own socket is bound to (the server computes it with its
    /// own `wake_target`, reading an unspecified bind address as loopback).
    pub fn to(target: SocketAddr) -> std::io::Result<Self> {
        let bind: SocketAddr = match target {
            SocketAddr::V4(_) => (std::net::Ipv4Addr::LOCALHOST, 0).into(),
            SocketAddr::V6(_) => (std::net::Ipv6Addr::LOCALHOST, 0).into(),
        };
        Ok(Self {
            socket: std::sync::Arc::new(UdpSocket::bind(bind)?),
            target,
        })
    }

    /// Ends the loop's current recv. Call it *after* the result is queued, so
    /// the woken loop finds it.
    pub fn wake(&self) {
        let _ = self.socket.send_to(&[], self.target);
    }
}
