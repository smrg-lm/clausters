//! An in-process audio server, by a **direct dependency on the server crate**.
//!
//! The standalone mode needs an audio server with no separate process and no
//! language client. The gui crate is part of the same ecosystem as the server
//! and the other clients, so under the optional `standalone` feature it simply
//! depends on the `clausters` crate (with `embed,realtime`) and constructs the
//! same in-process server the C ABI exposes — [`clausters::embed::Clausters`] —
//! through its direct Rust API: [`Clausters::open`] starts a full server (audio
//! device + engine + ring), [`Clausters::send`] delivers an OSC packet,
//! [`Clausters::poll_into`] pops a reply, and dropping it shuts the server down.
//!
//! Because pulling in the server (engine, cpal) is heavy, this is gated behind a
//! feature rather than always on: the default `clausters-gui` build stays light
//! (the size/packaging reason the gui is a separate crate in the first place),
//! and `--features standalone` opts into the embedded server. This is the
//! native-Rust counterpart of how the Python client reaches the same server over
//! the C ABI; here the dependency is a plain crate link, not an FFI load.

use clausters::embed::Clausters;

/// A live in-process server. Send it OSC, poll its replies; dropping it shuts
/// the server down (the `Clausters` `Drop` sends `/quit` and joins the thread).
pub struct EmbedServer {
    inner: Clausters,
}

impl EmbedServer {
    /// Starts an embedded server on the default audio device. The error carries
    /// the device/engine failure verbatim.
    pub fn open() -> Result<EmbedServer, String> {
        // 0 workers: the embedded server picks a sensible default.
        Ok(EmbedServer {
            inner: Clausters::open(0)?,
        })
    }

    /// Delivers one complete OSC packet to the embedded server. Returns `false`
    /// if the command ring was full (backpressure).
    pub fn send(&self, packet: &[u8]) -> bool {
        self.inner.send(packet)
    }

    /// Pops one pending reply into `buf`, returning its length, or `None` when no
    /// reply is pending. Replies larger than `buf` are dropped (use 64 KiB).
    pub fn poll_into(&self, buf: &mut [u8]) -> Option<usize> {
        self.inner.poll_into(buf)
    }
}
