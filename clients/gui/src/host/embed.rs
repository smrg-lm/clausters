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

use std::path::Path;
use std::sync::Arc;

use clausters::embed::Clausters;

use crate::host::BusSource;

/// A live in-process server. Send it OSC, poll its replies; dropping it shuts
/// the server down (the `Clausters` `Drop` sends `/server_quit` and joins the thread).
pub struct EmbedServer {
    inner: Clausters,
}

impl EmbedServer {
    /// Starts an embedded server on the default audio device, with no def store
    /// attached. The error carries the device/engine failure verbatim.
    pub fn open() -> Result<EmbedServer, String> {
        EmbedServer::open_with_data_dir(None)
    }

    /// Starts an embedded server that also loads the persisted defs at
    /// `data_dir` (SynthDefs, Faust defs, GraphDefs, MIDI bindings and the
    /// `boot.json` preset) before serving — how the standalone mode brings a
    /// whole bundle up from disk. `None` starts the server empty.
    pub fn open_with_data_dir(data_dir: Option<&Path>) -> Result<EmbedServer, String> {
        // 0 workers: the embedded server picks a sensible default.
        Ok(EmbedServer {
            inner: Clausters::open_with_data_dir(0, data_dir)?,
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

    /// A [`BusSource`] reading this server's own IPC
    /// segment — the in-process twin of mapping a `--shm` file.
    ///
    /// The data plane is the same one an out-of-process peer reads, so the host
    /// gets the clocks, the control buses, the per-bus levels and the audio
    /// taps with no messages at all. `head` picks which counter a playhead
    /// draws from: an editor reads the piece's position, a host watching a live
    /// server reads the device clock.
    ///
    /// `None` when the segment does not validate, which would mean this build's
    /// reader and the server it links disagree about the ABI — impossible in one
    /// binary, and reported rather than assumed away.
    #[cfg(unix)]
    pub fn bus_source(&self, head: crate::host::shm::HeadClock) -> Option<Arc<dyn BusSource>> {
        let segment = Arc::clone(self.inner.segment());
        let (base, size) = (segment.base(), segment.size());
        // SAFETY: `base`/`size` describe the very segment `segment` keeps
        // alive, and the `Arc` handed over as the owner is what holds it there
        // for as long as the view lives.
        let view = unsafe { crate::host::shm::SharedSegment::borrowed(base, size, segment) };
        match view {
            Ok(view) => Some(Arc::new(view.with_head(head))),
            Err(e) => {
                tracing::warn!("the embedded server's segment is unreadable: {e}");
                None
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::host::shm::HeadClock;
    use clausters_core::osc::{OscMessage, OscPacket, OscType, encode};

    /// The wire-up nothing else covers: an embedded server's own segment, read
    /// as a `BusSource`, reports **the piece's position** — so a window drawing
    /// a head from it draws where the piece is rather than how long the
    /// machine has been running.
    ///
    /// Skipped where no audio device can be opened, which is what a headless
    /// runner is: the boot failing is not this test's subject, and a session
    /// with no server is a case the host already handles out loud.
    #[test]
    fn the_embedded_segment_reads_the_pieces_position() {
        let Ok(embed) = EmbedServer::open() else {
            eprintln!("no audio device: skipping the embedded-segment read");
            return;
        };
        let bus = embed
            .bus_source(HeadClock::Piece)
            .expect("the segment this build wrote is the segment this build reads");
        let device = embed
            .bus_source(HeadClock::Device)
            .expect("the same segment, read on the other axis");

        let send = |addr: &str, args: Vec<OscType>| {
            let bytes = encode(&OscPacket::Message(OscMessage {
                addr: addr.into(),
                args,
            }))
            .expect("encodes");
            assert!(embed.send(&bytes), "the command ring took it");
        };
        // A group of its own, governed, so a stop freezes it and nothing else.
        send(
            "/group_new",
            vec![OscType::Int(77), OscType::Int(1), OscType::Int(0)],
        );
        send("/transport_group", vec![OscType::Int(77)]);
        send("/transport_locateSample", vec![OscType::Long(12_345)]);

        // **Waited for, not slept through.** The engine publishes once a block,
        // which is a fraction of a millisecond of work — but the first block
        // arrives when the *device* starts, and on a loaded machine that is
        // not within any fixed number of milliseconds. A single sleep made this
        // fail about one run in five, which is the worst kind of red: it says
        // nothing about the code and it trains a reader to re-run.
        assert!(
            settles(|| bus.sample_clock() == 12_345.0),
            "located, and stopped: the piece stands exactly where it was put \
             (read {})",
            bus.sample_clock()
        );
        assert!(
            device.sample_clock() > 0.0,
            "while the device clock, on the same segment, has been running all along"
        );

        send("/transport_play", vec![]);
        assert!(
            settles(|| bus.sample_clock() > 12_345.0),
            "and it moves once the transport rolls (read {})",
            bus.sample_clock()
        );
    }

    /// Whether `check` becomes true within a couple of seconds, polled.
    ///
    /// The deadline is generous and the poll is short, which is the shape a
    /// wait on another thread wants: it costs one poll interval when the
    /// machine is idle and it does not fail when the machine is not.
    fn settles(mut check: impl FnMut() -> bool) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if check() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        check()
    }
}

/// An in-process **on-demand session**: the same server with no audio device.
///
/// The peer of [`EmbedServer`], and the difference is what each one holds. That
/// one holds the machine's input and output; this one holds nothing but
/// computation and — given a segment path — the **samples**. An editor sends
/// it allocations, the editing verbs and renders, and lets a separate process
/// hold the devices and play what it owns.
///
/// The reason to separate them is not tidiness: an editor that owns its takes
/// through a real-time server holds an audio device it does not need, cannot
/// be restarted without taking the samples with it, and pays the whole
/// real-time surface to run three verbs.
pub struct EmbedSession {
    inner: clausters::embed::ClaustersSession,
}

impl EmbedSession {
    /// Opens a session whose samples live beside the segment at `shm`, at
    /// `sample_rate` and `channels`. A peer — this host included — maps every
    /// buffer it installs.
    pub fn open(shm: &Path, sample_rate: f64, channels: usize) -> Result<EmbedSession, String> {
        Ok(EmbedSession {
            inner: clausters::embed::ClaustersSession::open(
                &clausters::server::nrtsession::SessionConfig {
                    sample_rate,
                    channels,
                    shm: Some(shm.to_path_buf()),
                    ..Default::default()
                },
            )?,
        })
    }

    /// Delivers one complete OSC packet; `false` means the ring was full.
    pub fn send(&self, packet: &[u8]) -> bool {
        self.inner.send(packet)
    }

    /// Pops one pending reply into `buf`, returning its length.
    pub fn poll_into(&self, buf: &mut [u8]) -> Option<usize> {
        self.inner.poll_into(buf)
    }

    /// Where this session's segment is — what a player is pointed at and what
    /// the host maps its samples from.
    pub fn shm_path(&self) -> Option<&Path> {
        self.inner.shm_path()
    }
}
