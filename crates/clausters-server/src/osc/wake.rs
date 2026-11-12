//! Waking the command loop from a thread that is not it.
//!
//! [`OscServer::run`](super::server::OscServer::run) blocks in `recv_from`
//! under a read timeout, so anything produced *off* the network thread -- a TCP
//! frame, a MIDI message, a finished NRT job, a compiled Faust def -- is only
//! seen when that recv returns. A **zero-length UDP datagram** to the server's
//! own address ends the blocking recv immediately: the loop's own idle tick
//! stays a housekeeping interval instead of doubling as the latency of every
//! result.
//!
//! The [`Waker`] is the transport crate's, the one its stream hubs wake the
//! loop with; the producers inside the server process -- the NRT runner, the
//! Faust compiler, the MIDI input -- hold a clone of the same kind.

pub use clausters_net::Waker;
