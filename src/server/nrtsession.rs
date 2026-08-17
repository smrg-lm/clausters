//! The NRT server driven by commands instead of by a score — the mode an
//! editor works in.
//!
//! [`super::render`] is the batch side: it takes a whole score and returns a
//! file, start to end. An editor cannot work that way, because interaction is
//! not predictable — it answers a document, not a timeline. This module is the
//! same engine taking **operations on demand**.
//!
//! **There is no server clock.** Nothing advances between commands and nothing
//! can be scheduled against a running `now`: the only thing that moves time is
//! [`NrtSession::run`], and it moves exactly the frames it was asked for. Two
//! tiers follow from that and conflating them is the mistake this module exists
//! to prevent — a **buffer-editing** command has no timeline in any sense
//! (applying a gain to a span is not an event at an instant), while a **render**
//! operation has one *internally*: a self-contained score starting at 0 and
//! lasting the span. So determinism here is of **process, not of time**: the
//! same operation over the same material yields the samples it would yield
//! expressed in a score and rendered in batch, which is what `tests/nrt_session.rs`
//! asserts sample for sample.
//!
//! **The front is the embedded server's, reused rather than rebuilt**: an
//! in-memory segment, a headless [`OscServer`] at one end of its ring and an
//! [`IpcPeer`] at the other, exactly as `crate::embed` drives a server with no
//! audio device. That is where the transports, the clients, the replies,
//! `/server_sync` and the whole `/buffer_*` family come from. What is different
//! is only who owns the clock: here the caller does, and it hands it over one
//! operation at a time.
//!
//! **In this mode a pool buffer may be mutated in place** — the immutability
//! contract is a real-time rule, not a property of a buffer, and there is no
//! audio thread here to race. Nothing in this module does that yet; it is what
//! the editing verbs will use.
//!
//! No audio device is opened and none is needed: this builds and runs without
//! the `realtime` feature.

use std::sync::Arc;

use crate::dsp::Limits;
use crate::osc::server::{OscServer, ServerInfo};
use crate::server::engine::{
    BLOCK_SIZE, DEFAULT_AUDIO_BUSES, DEFAULT_CONTROL_BUSES, Engine, engine_pair_full,
};
#[cfg(unix)]
use crate::server::ipc::{DEFAULT_TAP_FRAMES, DEFAULT_TAPS};
use crate::server::ipc::{IpcPeer, Role, Segment};

/// How a session is opened. The defaults match the batch renderer's, so an
/// operation and a score of the same material start from the same server.
pub struct SessionConfig {
    pub sample_rate: f64,
    pub channels: usize,
    /// DSP helper threads for `/group_parallel` groups; 0 runs everything on
    /// the calling thread.
    pub workers: usize,
    /// Starting seed for the stochastic UGens. `None` takes a fresh one — the
    /// same choice `RenderConfig::seed` offers, and the same reason to pin it:
    /// an operation is only repeatable if its seed is.
    pub seed: Option<u64>,
    pub audio_buses: usize,
    pub control_buses: usize,
    pub limits: Limits,
    /// Where to put the segment, when this session's material is to be
    /// **shared**: a path makes it a mapped file, so a second process can read
    /// the directory and map every buffer this session holds (S19). `None` is
    /// the ordinary in-process session, whose segment lives on the heap.
    ///
    /// It is here rather than assumed because it is a *deployment* choice: an
    /// on-demand server that renders and edits material an editor draws wants
    /// it; one answering a script inside one process has nobody to share with.
    pub shm: Option<std::path::PathBuf>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000.0,
            channels: 2,
            workers: 0,
            seed: None,
            audio_buses: DEFAULT_AUDIO_BUSES,
            control_buses: DEFAULT_CONTROL_BUSES,
            limits: Limits::default(),
            shm: None,
        }
    }
}

/// A server that executes operations on demand, with no audio device and no
/// clock of its own. See the module docs.
pub struct NrtSession {
    server: OscServer,
    engine: Engine,
    peer: IpcPeer,
    /// Kept alive for both peers; the ring lives in it, and it is what a
    /// caller hands a player so the two share one material.
    segment: Arc<Segment>,
    /// The segment file this session **created**, and therefore has to remove:
    /// a session's material is the session's, and a segment left behind in
    /// `/dev/shm` after the editor is gone is a leak with a take in it. `None`
    /// when the segment is on the heap or was somebody else's already.
    owned_segment: Option<std::path::PathBuf>,
    channels: usize,
    sample_rate: f64,
    /// The one block the engine renders into, allocated once.
    block: Vec<f32>,
    /// Frames run so far. Not a clock anyone can schedule against: it is the
    /// sum of what the caller has asked for, reported so an operation can say
    /// where it happened.
    frames: u64,
    /// The seed this session resolved, reported for the same reason a render
    /// reports its own.
    seed: u64,
}

impl NrtSession {
    /// Opens a session. Nothing runs until [`Self::run`] is called.
    pub fn open(cfg: &SessionConfig) -> Result<Self, String> {
        if cfg.channels == 0 {
            return Err("channels must be at least 1".into());
        }
        if !(cfg.sample_rate.is_finite() && cfg.sample_rate > 0.0) {
            return Err("sample rate must be positive".into());
        }
        // Both modes flush denormals, so an operation and a batch render of the
        // same material stay sample-identical (the rule `render` follows).
        crate::dsp::denormals::flush_to_zero();

        // Set when this session *created* the file, which is what decides
        // whether it is this session's to remove on the way out.
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut owned_segment: Option<std::path::PathBuf> = None;
        // A path makes the segment a file, which is the whole difference
        // between a session nobody else can see and one whose buffers a peer
        // maps by name.
        let segment = match &cfg.shm {
            #[cfg(not(unix))]
            Some(path) => {
                // A shared segment is a mapped file, and off Unix there is
                // none to map. Refused rather than silently opening a session
                // whose material nobody else can reach.
                return Err(format!(
                    "shared material needs a Unix segment; {} cannot be opened here",
                    path.display()
                ));
            }
            #[cfg(unix)]
            Some(path) => {
                // Sized with the ordinary tap region even though a session
                // never writes one: the process that attaches to this segment
                // *does* have a device, and its scopes and meters need the
                // rings to be there.
                let (segment, created) = Segment::open_or_create_full(
                    path,
                    cfg.control_buses,
                    DEFAULT_TAPS,
                    DEFAULT_TAP_FRAMES,
                )
                .map_err(|e| {
                    format!("cannot open the shared segment at {}: {e}", path.display())
                })?;
                // A session is driven through the ring, so it has to be the
                // one serving it — and it owns the material it publishes.
                // Finding the command plane taken means another server is
                // already the owner here, which is a wiring mistake worth
                // saying out loud rather than half-working.
                if !segment.claim_control() {
                    return Err(format!(
                        "the segment at {} is already served by pid {}",
                        path.display(),
                        segment.control_owner().unwrap_or(0),
                    ));
                }
                owned_segment = created.then(|| path.clone());
                segment
            }
            None => Segment::in_memory_with(cfg.control_buses),
        };
        let (mut engine, handle) = engine_pair_full(
            cfg.sample_rate as f32,
            cfg.channels,
            cfg.workers,
            Some(Arc::clone(&segment)),
            cfg.audio_buses,
            cfg.control_buses,
            cfg.limits,
        );
        // **The clocks belong to the device, and this mode has none.** The
        // frames a session runs are what an operation asked for, not time
        // passing, so publishing them into the segment would report a playhead
        // that moves whenever somebody applies a fade — and in the arrangement
        // this mode exists for, the process that *does* have a device is
        // writing those very words from another process.
        engine.silence_time_publication();
        let info = ServerInfo {
            nominal_sample_rate: cfg.sample_rate,
            actual_sample_rate: cfg.sample_rate,
        };
        // Headless: no socket, and a sample clock rather than a wall one —
        // the same `TimeSource` the offline drive already uses, because a
        // session that answered `/clock_query` with the wall clock would be
        // reporting a time nothing here advances.
        let mut server = OscServer::headless(info, handle, 0.0);
        let seed = cfg.seed.unwrap_or_else(clausters_core::rng::entropy_seed);
        server.set_seed(seed);
        // This driver owns the clock, so `/buffer_render` is legal here — and
        // only here.
        server.enable_offline_renders();
        server
            .attach_ipc(IpcPeer::new(Arc::clone(&segment), Role::Server))
            .map_err(|e| e.to_string())?;
        if let Some(path) = &cfg.shm {
            // With a segment on disk, every buffer this session installs lives
            // in a region beside it — which is what lets an editor draw and
            // write the material of a server that has no audio device at all.
            server.share_buffers_at(path.clone());
        }
        Ok(Self {
            server,
            engine,
            peer: IpcPeer::new(Arc::clone(&segment), Role::Client),
            segment,
            owned_segment,
            channels: cfg.channels,
            sample_rate: cfg.sample_rate,
            block: vec![0.0; BLOCK_SIZE * cfg.channels],
            frames: 0,
            seed,
        })
    }

    /// The segment this session publishes into: its material directory, its
    /// control buses, and the rings it serves.
    pub fn segment(&self) -> &Arc<Segment> {
        &self.segment
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Frames run so far, across every operation.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// The seed the session resolved at open.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Delivers one complete OSC packet. `false` means the ring was
    /// momentarily full — retry after [`Self::settle`].
    pub fn send(&self, packet: &[u8]) -> bool {
        self.peer.push(crate::server::ipc::DEFAULT_PEER, packet)
    }

    /// Encodes and delivers one message. The convenience the callers of this
    /// module actually want; `send` stays for a packet already built.
    pub fn send_msg(&self, addr: &str, args: Vec<rosc::OscType>) -> Result<bool, String> {
        let bytes = rosc::encoder::encode(&rosc::OscPacket::Message(rosc::OscMessage {
            addr: addr.into(),
            args,
        }))
        .map_err(|e| format!("{addr}: {e}"))?;
        Ok(self.send(&bytes))
    }

    /// Pops one pending reply into `buf`, returning its length.
    pub fn poll_into(&self, buf: &mut [u8]) -> Option<usize> {
        self.peer.try_pop(buf).map(|(_, len)| len)
    }

    /// Serves what has arrived **without advancing time**: one turn of the
    /// server (the ring drained, commands dispatched, buffer jobs run inline,
    /// their results collected) and then the engine applying them.
    ///
    /// The second half is the part a real-time driver gets for free: a
    /// completed `/buffer_alloc` reaches the pool as `Cmd::SetBuffer` and only
    /// the engine can install it, which in real time the next block does a
    /// millisecond later. Here there is no next block until an operation asks
    /// for one, so the engine drains explicitly — the alternative being to
    /// process a block nobody wanted, which is the clock this mode is defined
    /// as not having.
    ///
    /// Returns true once a `/server_quit` has arrived.
    ///
    /// This is also where a queued `/buffer_render` is performed, because this
    /// is the only place that holds both halves: the server parsed it and will
    /// answer it, and the engine here is the one that has to run.
    pub fn settle(&mut self) -> bool {
        let quit = self.server.step();
        self.engine.drain();
        while let Some(req) = self.server.take_offline_render() {
            let outcome = self.perform_render(req.index, req.frames);
            self.server.finish_offline_render(req, outcome);
            // The install is a command like any other, so the engine has to
            // take it before the next operation reads that buffer.
            self.engine.drain();
        }
        quit
    }

    /// Runs the graph for `frames` and installs the result in buffer `index` —
    /// the body of `/buffer_render`, and reachable directly for a caller that
    /// is already in Rust.
    ///
    /// What lands is `frames` frames of the first [`Self::channels`] output
    /// buses, and it **replaces** what the index held rather than being laid
    /// into it — the operation's own length and width are what they are, and
    /// fitting them into a shape allocated earlier would mean either truncating
    /// a render or leaving half a buffer stale. The index must already be
    /// allocated, which is the caller saying that slot is the one they mean.
    pub fn perform_render(&mut self, index: usize, frames: u64) -> Result<(), String> {
        let samples = self.run_to_vec(frames)?;
        let channels = self.channels;
        let buffer =
            crate::dsp::buffer::Buffer::new(samples, channels, frames as usize, self.sample_rate);
        self.server.install_buffer(index, Arc::new(buffer))
    }

    /// [`Self::settle`] up to `turns` times, stopping early once the ring is
    /// quiet. Async work that crosses a thread (a Faust compile) needs more
    /// than one turn, and a caller that has just sent a batch has no way to
    /// know how many.
    pub fn settle_for(&mut self, turns: usize) -> bool {
        let mut quit = false;
        for _ in 0..turns.max(1) {
            quit |= self.settle();
        }
        quit
    }

    /// Runs `frames` frames of the engine, handing each block to `sink`
    /// (interleaved, the last one truncated to the requested length).
    ///
    /// **This is the operation, and it is closed**: no command is accepted
    /// while it runs, deliberately. Serving the ring mid-operation would let a
    /// message land at a frame that depends on how fast the caller happened to
    /// be, which is exactly the difference between this and a batch render that
    /// the mode promises there is none of. Send, [`Self::settle`], then run.
    pub fn run(
        &mut self,
        frames: u64,
        mut sink: impl FnMut(&[f32]) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut done = 0u64;
        while done < frames {
            self.engine.process_block(&mut self.block);
            let take = (frames - done).min(BLOCK_SIZE as u64) as usize;
            sink(&self.block[..take * self.channels])?;
            done += BLOCK_SIZE as u64;
            self.frames += BLOCK_SIZE as u64;
        }
        Ok(())
    }

    /// [`Self::run`] into a fresh interleaved buffer — the shape an operation
    /// that has to hand its samples back wants.
    pub fn run_to_vec(&mut self, frames: u64) -> Result<Vec<f32>, String> {
        let mut out = Vec::with_capacity(frames as usize * self.channels);
        self.run(frames, |chunk| {
            out.extend_from_slice(chunk);
            Ok(())
        })?;
        Ok(out)
    }
}

impl Drop for NrtSession {
    fn drop(&mut self) {
        // A shared session gives the command plane back, so the next process
        // on this segment adopts it instead of taking it over from a pid that
        // is gone. On a heap segment this is a no-op: nobody else can see it.
        self.segment.release_control();
        // And a session that created its segment takes it with it, regions
        // and all. Unlinking leaves every mapping a player still holds valid
        // until it drops it — the same property freeing one buffer relies on —
        // so this ends the material rather than pulling it out from under
        // somebody.
        let Some(path) = self.owned_segment.take() else {
            return;
        };
        if let Some(dir) = path.parent()
            && let Some(prefix) = path.file_name().and_then(|n| n.to_str())
            && let Ok(entries) = std::fs::read_dir(dir)
        {
            let region = format!("{prefix}.buf");
            for entry in entries.flatten() {
                if entry
                    .file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with(&region))
                {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        let _ = std::fs::remove_file(&path);
    }
}
