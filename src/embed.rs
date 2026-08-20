//! the embed C ABI (feature `embed`) — Clausters as a library.
//!
//! The cdylib (`libclausters.so` / `.dylib` / `.dll`) is the **canonical
//! language-agnostic surface**: thin bindings in any language sit on top of
//! it (Python via stdlib `ctypes` in `clients/python/clausters.py`,
//! JavaScript via Node/Deno FFI later). The boundary follows the project
//! rule: only **basic structures** cross it — flat `f32` arrays as
//! pointer + length, integers, NUL-terminated error strings. Never a
//! library type: a numpy array can *view* the returned pointer without
//! copying, but that is the client's choice, not a dependency.
//!
//! Two entry points:
//!
//! - [`clausters_render`]: the synchronous "scientific" call — render a
//!   binary score offline and get the interleaved samples back. No audio
//!   device, no threads, no asynchrony; blocks the *caller* only.
//! - `clausters_open`/`clausters_send`/`clausters_poll`: a full live
//!   server in-process. Commands are ordinary OSC packets delivered by
//!   function call through the same heap-backed ring the `--shm` transport
//!   uses (`server::ipc`); replies are polled. The data plane is direct:
//!   `clausters_clock` and `clausters_ctl_set`/`clausters_ctl_get`
//!   touch the segment atomics with no command round trip at all.
//!
//! Versioning: check [`clausters_abi_version`] before anything else; the
//! constant moves in lockstep with the segment layout version (the scsynth
//! plugin-ABI lesson: every binary boundary is versioned and checked).

#![cfg(feature = "embed")]

#[cfg(feature = "realtime")]
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
#[cfg(feature = "realtime")]
use std::thread::JoinHandle;

use crate::server::ipc::{ABI_VERSION, IpcPeer, Role, Segment};
use crate::server::render::{RenderConfig, Score, render_to_vec};

/// The C ABI version (== the IPC segment layout version).
#[unsafe(no_mangle)]
pub extern "C" fn clausters_abi_version() -> u32 {
    ABI_VERSION
}

/// Writes `msg` into (`buf`, `cap`) as a NUL-terminated C string.
fn write_error(msg: &str, buf: *mut u8, cap: usize) {
    if buf.is_null() || cap == 0 {
        return;
    }
    let bytes = msg.as_bytes();
    let n = bytes.len().min(cap - 1);
    // SAFETY: caller-provided buffer of at least `cap` bytes.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, n);
        *buf.add(n) = 0;
    }
}

/// Renders a binary score (the `--nrt` format: length-prefixed OSC packets,
/// timetags in seconds from the start) synchronously.
///
/// `seed` starts the render's stochastic UGens: pass **NULL for a fresh take**
/// (the default — a random process is unpredictable first), or a pointer to
/// the seed of a take you want repeated. Either way the seed actually used
/// comes back in `out_seed`, which is what makes the take repeatable at all.
/// See [`crate::server::render::RenderConfig::seed`].
///
/// On success returns a malloc'd interleaved `f32` buffer and writes the
/// frame count to `out_frames` (total samples = frames × channels), the
/// number of score events executed to `out_events` and the render's seed to
/// `out_seed`; free the buffer with [`clausters_free_samples`]. On failure
/// returns NULL and writes a human-readable message into (`err`, `err_cap`).
///
/// # Safety
/// `score`/`score_len` must describe a readable byte range; `seed` must be
/// NULL or point to a readable `u64`; `out_frames`, `out_events` and
/// `out_seed` must be writable; `err` either NULL or writable for `err_cap`
/// bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_render(
    score: *const u8,
    score_len: usize,
    sample_rate: f64,
    channels: u32,
    workers: u32,
    seed: *const u64,
    out_frames: *mut u64,
    out_events: *mut u64,
    out_seed: *mut u64,
    err: *mut u8,
    err_cap: usize,
) -> *mut f32 {
    let bytes = if score.is_null() {
        &[][..]
    } else {
        // SAFETY: caller contract.
        unsafe { std::slice::from_raw_parts(score, score_len) }
    };
    // SAFETY: caller contract — NULL means "draw one".
    let seed = if seed.is_null() {
        None
    } else {
        Some(unsafe { *seed })
    };
    let result = Score::from_bytes(bytes).and_then(|score| {
        let cfg = RenderConfig {
            sample_rate,
            channels: channels as usize,
            workers: workers as usize,
            seed,
            // The embed ABI has no capacity arguments, so an embedded render
            // takes the defaults. Raising them would mean widening the C ABI,
            // which moves ABI_VERSION -- deferred until something needs it.
            ..RenderConfig::default()
        };
        render_to_vec(&score, &cfg)
    });
    match result {
        Ok((samples, stats)) => {
            // SAFETY: caller contract.
            unsafe {
                *out_frames = stats.frames;
                *out_events = stats.events as u64;
                *out_seed = stats.seed;
            }
            let mut samples = samples.into_boxed_slice();
            let ptr = samples.as_mut_ptr();
            std::mem::forget(samples);
            ptr
        }
        Err(e) => {
            write_error(&e, err, err_cap);
            std::ptr::null_mut()
        }
    }
}

/// Frees a buffer returned by [`clausters_render`]. `samples` is
/// frames × channels (the full length, not per channel).
///
/// # Safety
/// Must be called exactly once with the pointer and total sample count of
/// one successful `clausters_render`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_free_samples(ptr: *mut f32, samples: u64) {
    if !ptr.is_null() {
        // SAFETY: reconstructs the Box from clausters_render.
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, samples as usize)) });
    }
}

/// An in-process server in **pulled mode**: engine + serving logic with
/// **no device, no sockets and no threads** — the host owns the audio thread
/// and calls [`ClaustersHeadless::process_block`] itself, callback-style.
/// This is the native face of the browser build (the AudioWorklet calls
/// exactly this from its render quantum) and a supported embed mode in its
/// own right: a plugin or another host that already has an audio callback
/// embeds the whole server this way.
///
/// The wiring is the embed model minus cpal and minus the socket loop:
/// commands are complete OSC packets pushed into the in-memory ring
/// ([`ClaustersHeadless::send`]), replies are pulled from the reply ring
/// ([`ClaustersHeadless::poll_into`]), and each `process_block` first runs
/// one serving turn (`OscServer::step`: drain the ring, pump `/bus_stream`/
/// `/bus_tapStream`, collect async results) — so everything the socket server
/// does, paced by the host's own callback. NRT jobs run inline on the
/// calling thread, and stream periods/timetags follow the **engine sample
/// clock** (deterministic under offline drive; see `OscServer::headless`).
///
/// Not RT-strict: the serving turn allocates (translate, NRT) on the calling
/// thread, the accepted relaxation of this mode — a host that needs the
/// native no-alloc audio callback uses `Clausters` (its own threads) or
/// the full server instead.
pub struct ClaustersHeadless {
    engine: crate::server::engine::Engine,
    server: crate::osc::server::OscServer,
    peer: IpcPeer,
    segment: Arc<Segment>,
    channels: usize,
    quit: bool,
}

impl ClaustersHeadless {
    /// Builds the pulled server: `sample_rate`/`channels` are the host
    /// callback's format; `unix_epoch` (Unix seconds at sample 0) anchors
    /// the sample axis for wall-clocked clients' timetags — pass the current
    /// time for live use, any fixed value for deterministic runs.
    pub fn new(sample_rate: f64, channels: usize, unix_epoch: f64) -> Result<Self, String> {
        use crate::osc::server::{OscServer, ServerInfo};

        let segment = Segment::in_memory();
        let (engine, handle) = crate::server::engine::engine_pair_full(
            sample_rate as f32,
            channels,
            0, // workers: sequential, bit-identical — the only wasm mode
            Some(Arc::clone(&segment)),
            crate::server::engine::DEFAULT_AUDIO_BUSES,
            crate::server::engine::DEFAULT_CONTROL_BUSES,
            crate::dsp::Limits::default(),
        );
        let info = ServerInfo {
            nominal_sample_rate: sample_rate,
            actual_sample_rate: sample_rate,
        };
        let mut server = OscServer::headless(info, handle, unix_epoch);
        server
            .attach_ipc(IpcPeer::new(Arc::clone(&segment), Role::Server))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            engine,
            server,
            peer: IpcPeer::new(Arc::clone(&segment), Role::Client),
            segment,
            channels,
            quit: false,
        })
    }

    /// Sets the ceiling on the bus indices one `/bus_stream` subscription may
    /// list, the pulled server's half of the native `--max-stream-buses`
    /// (default [`crate::osc::DEFAULT_MAX_STREAM_BUSES`]).
    ///
    /// It is a setter rather than a constructor argument because this engine
    /// boots from an audio callback's format and nothing else: an embedder
    /// that wants another ceiling — a page whose document holds hundreds of
    /// live canvases — says so before it starts serving. What a client then
    /// gets is this clamped by the ring it reads over, which is the number
    /// `/server_query.reply` hands it.
    pub fn set_max_stream_buses(&mut self, n: usize) {
        self.server.set_max_stream_buses(n);
    }

    /// Delivers one complete OSC packet (message or bundle) through the
    /// command ring; it takes effect on the next [`Self::process_block`].
    /// Returns `false` when the ring is momentarily full (backpressure).
    ///
    /// Sends as [`ipc::DEFAULT_PEER`](crate::server::ipc::DEFAULT_PEER) — the
    /// single client a segment used to have. An embedder carrying **several**
    /// independent clients (a page whose script and GUI host share one engine)
    /// gives each its own tag with [`Self::send_as`], which is what keeps their
    /// subscriptions and their replies apart.
    pub fn send(&self, packet: &[u8]) -> bool {
        self.send_as(crate::server::ipc::DEFAULT_PEER, packet)
    }

    /// [`Self::send`], authored by `peer`. The tag is the embedder's to assign:
    /// there is no handshake on the ring and none is needed — the server only
    /// has to tell its clients apart, not name them.
    pub fn send_as(&self, peer: u32, packet: &[u8]) -> bool {
        self.peer.push(peer, packet)
    }

    /// Pops one pending reply into `buf`, returning its length, or `None`
    /// when none is pending. A reply larger than `buf` is dropped (use
    /// 64 KiB).
    pub fn poll_into(&self, buf: &mut [u8]) -> Option<usize> {
        self.poll_from(buf).map(|(_, len)| len)
    }

    /// [`Self::poll_into`], also reporting **who the reply is for**: the peer
    /// tag of the client that asked. An embedder with one client can ignore
    /// it; one with several routes on it, which is the whole point of the tag.
    /// A notification that reaches several clients arrives as several replies,
    /// one per tag, so the router never has to fan anything out itself.
    pub fn poll_from(&self, buf: &mut [u8]) -> Option<(u32, usize)> {
        self.peer.try_pop(buf)
    }

    /// Renders into `out` (interleaved, length a multiple of
    /// `BLOCK_SIZE * channels`): a serving turn (`OscServer::step`) before
    /// **each** engine block, so stream pacing and async results keep their
    /// per-block cadence however large the pulled buffer is. The host's
    /// audio callback calls this once per buffer.
    pub fn process_block(&mut self, out: &mut [f32]) -> Result<(), String> {
        let block = crate::server::engine::BLOCK_SIZE * self.channels;
        if block == 0 || !out.len().is_multiple_of(block) {
            return Err(format!(
                "output length {} is not a multiple of BLOCK_SIZE * channels ({block})",
                out.len()
            ));
        }
        for chunk in out.chunks_exact_mut(block) {
            if self.server.step() {
                self.quit = true;
            }
            self.engine.process_block(chunk);
        }
        Ok(())
    }

    /// Whether a `/server_quit` has arrived. The pulled server has no loop to end,
    /// so quitting is the host's decision: it reads this and stops calling
    /// [`Self::process_block`] (dropping the value releases everything).
    pub fn quit_requested(&self) -> bool {
        self.quit
    }

    /// The engine's sample counter (block-accurate).
    pub fn clock(&self) -> u64 {
        self.segment.clock().load(Ordering::Acquire)
    }

    /// Writes a control bus directly in the data plane (no command round
    /// trip), exactly like the C ABI's `clausters_ctl_set`.
    pub fn ctl_set(&self, index: usize, value: f32) {
        self.segment.control_buses().set(index, value);
    }

    /// Reads a control bus from the data plane.
    pub fn ctl_get(&self, index: usize) -> f32 {
        self.segment.control_buses().get(index)
    }

    /// Installs host-provided samples as buffer `index` (interleaved,
    /// `data.len() = frames * channels`): the browser's `/buffer_allocRead`
    /// replacement — the page fetches and decodes (Web Audio's
    /// `decodeAudioData`), then hands the engine the samples. Runs on the
    /// calling thread through the same install path as the async `/buffer_*`
    /// commands, so `/buffer_query` and the def machinery see it identically.
    pub fn buffer_load(
        &mut self,
        index: usize,
        channels: usize,
        sample_rate: f64,
        data: &[f32],
    ) -> Result<(), String> {
        if channels == 0 || !data.len().is_multiple_of(channels) {
            return Err(format!(
                "data length {} is not a multiple of {channels} channels",
                data.len()
            ));
        }
        let frames = data.len() / channels;
        let buffer = Arc::new(crate::dsp::buffer::Buffer::new(
            data.to_vec(),
            channels,
            frames,
            sample_rate,
        ));
        self.server.install_buffer(index, buffer)
    }
}

/// An in-process **on-demand session**: engine, network loop and buffers,
/// with **no audio device** — the mode an editor works in.
///
/// `Clausters` below is the other door and the difference is the whole point:
/// that one is a full real-time server, holding the machine's input and
/// output. (Named rather than linked: it is behind the `realtime` feature, and
/// a link would resolve only in the builds that compile it in.) This one holds nothing but computation. It performs the editing
/// verbs, renders on demand ([`/buffer_render`](crate::server::nrtsession)),
/// and — given a `shm` path — **owns the buffers**: every buffer it installs
/// lives in a region beside the segment, where a peer draws it, a peer edits
/// it, and a separate RT server plays it.
///
/// That separation is what a host needs to be an application rather than a
/// window on a server: the editor and its buffers outlive the process that
/// happens to be making sound, and killing the player takes no take with it.
///
/// It is driven exactly like `Clausters` — [`send`](Self::send) an OSC packet,
/// [`poll_into`](Self::poll_into) a reply — so a caller swaps one for the
/// other without learning a second protocol. The session runs on its own
/// thread, serving the ring and performing what arrives; dropping this stops
/// it.
pub struct ClaustersSession {
    peer: IpcPeer,
    segment: Arc<Segment>,
    shm: Option<std::path::PathBuf>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ClaustersSession {
    /// Opens a session against `cfg`. With `cfg.shm` set, the segment is a
    /// file and this session is the **owner** of everything in it; without
    /// one it is an ordinary in-process session nobody else can see.
    pub fn open(cfg: &crate::server::nrtsession::SessionConfig) -> Result<Self, String> {
        let mut session = crate::server::nrtsession::NrtSession::open(cfg)?;
        let segment = Arc::clone(session.segment());
        let peer = IpcPeer::new(Arc::clone(&segment), Role::Client);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("clausters-session".into())
            .spawn(move || {
                // One turn per pass, then a short sleep when nothing came:
                // this mode has no clock, so there is nothing to pace against
                // and nothing to be late for — only work to pick up.
                while !flag.load(Ordering::Relaxed) {
                    if session.settle() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            })
            .map_err(|e| format!("cannot start the session thread: {e}"))?;
        Ok(Self {
            peer,
            segment,
            shm: cfg.shm.clone(),
            stop,
            thread: Some(thread),
        })
    }

    /// Delivers one complete OSC packet; `false` means the ring was full.
    pub fn send(&self, packet: &[u8]) -> bool {
        self.peer.push(crate::server::ipc::DEFAULT_PEER, packet)
    }

    /// Pops one pending reply into `buf`, returning its length.
    pub fn poll_into(&self, buf: &mut [u8]) -> Option<usize> {
        self.peer.try_pop(buf).map(|(_, len)| len)
    }

    /// The segment this session publishes into — the buffers' directory,
    /// the control buses, and the clocks somebody *else* writes.
    pub fn segment(&self) -> &Arc<Segment> {
        &self.segment
    }

    /// Where the segment is, when it is a file: what a player is pointed at
    /// (`clausters --shm <path>`) and what a peer maps buffers by.
    pub fn shm_path(&self) -> Option<&std::path::Path> {
        self.shm.as_deref()
    }
}

impl Drop for ClaustersSession {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// An in-process live server: audio device + engine + network loop, with
/// the host as the single ring client.
///
/// This is also the **direct Rust API** behind the C ABI: a Rust embedder
/// (the GUI host's standalone mode, for one) constructs it with
/// [`Clausters::open`] and drives it with [`Clausters::send`]/
/// [`Clausters::poll_into`], dropping it to shut the server down — the same
/// in-process server the `clausters_open`/`_send`/`_poll`/`_close` C exports
/// wrap thinly for non-Rust callers.
#[cfg(feature = "realtime")]
pub struct Clausters {
    peer: IpcPeer,
    /// The cpal stream lives here; dropping it stops audio.
    _backend: crate::server::backend::AudioBackend,
    server: Option<JoinHandle<std::io::Result<()>>>,
    segment: Arc<Segment>,
}

#[cfg(feature = "realtime")]
impl Clausters {
    /// Opens the default audio device and starts a full server in-process
    /// (`workers` engine helper threads; 0 picks a sensible default). The
    /// returned handle owns the audio stream and the network thread; dropping
    /// it shuts the server down. No def store is attached — use
    /// [`Clausters::open_with_data_dir`] to also load persisted defs.
    pub fn open(workers: usize) -> Result<Clausters, String> {
        Clausters::open_with_data_dir(workers, None)
    }

    /// Like [`Clausters::open`] but also attaches the on-disk def store at
    /// `data_dir`, loading whatever it holds before the server starts serving:
    /// persisted SynthDefs, Faust defs (with the `faust` feature), GraphDefs,
    /// MIDI bindings and the `boot.json` preset — the same startup the
    /// standalone server binary performs. This is how the GUI's standalone mode
    /// brings a whole bundle up from a data directory. `None` keeps the server
    /// empty. A store that cannot be opened is logged and skipped, not fatal.
    pub fn open_with_data_dir(
        workers: usize,
        data_dir: Option<&Path>,
    ) -> Result<Clausters, String> {
        use crate::osc::server::{OscServer, ServerInfo};

        let segment = Segment::in_memory();
        // Embedded hosts follow the device's default rate (None); they can
        // resample on their side if they need a specific rate. Default bus
        // counts (the in-memory segment is sized to match).
        let (backend, handle) = crate::server::backend::start(
            workers,
            Some(Arc::clone(&segment)),
            None,
            crate::server::engine::DEFAULT_AUDIO_BUSES,
            crate::server::engine::DEFAULT_CONTROL_BUSES,
            crate::dsp::Limits::default(),
            None,
            0,
            // An embedded server takes the machine's default devices under
            // whatever name the audio graph gives it: naming ports is a
            // deployment choice, and this one is inside somebody else's
            // process.
            &crate::server::backend::Devices::default(),
        )
        .map_err(|e| e.to_string())?;
        let info = ServerInfo {
            nominal_sample_rate: backend.sample_rate as f64,
            actual_sample_rate: backend.sample_rate as f64,
        };
        // The socket is an ephemeral localhost port: unused by the embed
        // client (commands go through the ring), it just drives the loop's
        // tick — and doubles as an escape hatch for debugging.
        let mut server =
            OscServer::bind(("127.0.0.1", 0), info, handle).map_err(|e| e.to_string())?;
        server
            .attach_ipc(IpcPeer::new(Arc::clone(&segment), Role::Server))
            .map_err(|e| e.to_string())?;
        if let Some(dir) = data_dir {
            match crate::server::defstore::DefStore::open(dir) {
                Ok(store) => {
                    server.attach_store(store);
                    tracing::info!("embed: loaded persisted defs from {}", dir.display());
                }
                Err(e) => tracing::warn!(
                    "embed: def store at {} unavailable, starting empty: {e}",
                    dir.display()
                ),
            }
        }
        let thread = std::thread::Builder::new()
            .name("clausters-embed-server".into())
            .spawn(move || server.run())
            .expect("failed to spawn the embedded server thread");
        Ok(Clausters {
            peer: IpcPeer::new(Arc::clone(&segment), Role::Client),
            _backend: backend,
            server: Some(thread),
            segment,
        })
    }

    /// The IPC segment this server publishes into — the same data plane a
    /// `--shm` server writes to a file, here in memory.
    ///
    /// An in-process host reads the clocks, the control buses, the per-bus
    /// levels and the audio taps straight out of it, exactly as an
    /// out-of-process peer reads the mapped file, instead of asking for them
    /// over the ring. Handing out the `Arc` rather than a pointer is what keeps
    /// the memory alive for as long as a reader holds it.
    pub fn segment(&self) -> &Arc<Segment> {
        &self.segment
    }

    /// Delivers one complete OSC packet (message or bundle) through the command
    /// ring. Returns `false` when the ring is momentarily full (backpressure).
    ///
    /// Sends as [`ipc::DEFAULT_PEER`](crate::server::ipc::DEFAULT_PEER); see
    /// `LiveEngine::send_as` for why an embedder would want another tag (that
    /// type is behind the `embed` feature, so it is named rather than linked).
    pub fn send(&self, packet: &[u8]) -> bool {
        self.send_as(crate::server::ipc::DEFAULT_PEER, packet)
    }

    /// [`Self::send`], authored by `peer`.
    pub fn send_as(&self, peer: u32, packet: &[u8]) -> bool {
        self.peer.push(peer, packet)
    }

    /// Pops one pending reply into `buf`, returning its length, or `None` when
    /// none is pending. A reply larger than `buf` is dropped (use 64 KiB).
    pub fn poll_into(&self, buf: &mut [u8]) -> Option<usize> {
        self.poll_from(buf).map(|(_, len)| len)
    }

    /// [`Self::poll_into`], also reporting who the reply is for (see
    /// `LiveEngine::poll_from`, behind the `embed` feature).
    pub fn poll_from(&self, buf: &mut [u8]) -> Option<(u32, usize)> {
        self.peer.try_pop(buf)
    }
}

#[cfg(feature = "realtime")]
impl Drop for Clausters {
    fn drop(&mut self) {
        // Sends `/server_quit` through the ring and joins the network thread (the cpal
        // stream stops when `_backend` drops after this). Same shutdown the C
        // ABI's `clausters_close` used to do inline.
        let quit = rosc::encoder::encode(&rosc::OscPacket::Message(rosc::OscMessage {
            addr: "/server_quit".into(),
            args: vec![],
        }))
        .expect("static /server_quit message encodes");
        let _ = self.peer.push(crate::server::ipc::DEFAULT_PEER, &quit);
        if let Some(thread) = self.server.take() {
            let _ = thread.join();
        }
    }
}

/// Opens the default audio device and starts a full server in-process.
/// Returns NULL on failure (the error goes to `err`). Close with
/// [`clausters_close`]. Thin C wrapper over [`Clausters::open`].
///
/// # Safety
/// `err` either NULL or writable for `err_cap` bytes.
#[cfg(feature = "realtime")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_open(
    workers: u32,
    err: *mut u8,
    err_cap: usize,
) -> *mut Clausters {
    match Clausters::open(workers as usize) {
        Ok(c) => Box::into_raw(Box::new(c)),
        Err(e) => {
            write_error(&e, err, err_cap);
            std::ptr::null_mut()
        }
    }
}

/// Delivers one complete OSC packet (message or bundle). Returns 0 on
/// success, -1 when the command ring is full (backpressure: retry).
///
/// # Safety
/// `handle` from [`clausters_open`]; `packet`/`len` a readable byte range.
#[cfg(feature = "realtime")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_send(
    handle: *mut Clausters,
    packet: *const u8,
    len: usize,
) -> i32 {
    let Some(h) = (unsafe { handle.as_ref() }) else {
        return -1;
    };
    let bytes = unsafe { std::slice::from_raw_parts(packet, len) };
    if h.send(bytes) { 0 } else { -1 }
}

/// Pops one pending reply into (`buf`, `cap`). Returns the packet length,
/// 0 when none is pending, or -1 on error. Replies bigger than `cap` are
/// dropped (use 64 KiB to be safe).
///
/// # Safety
/// `handle` from [`clausters_open`]; `buf` writable for `cap` bytes.
#[cfg(feature = "realtime")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_poll(handle: *mut Clausters, buf: *mut u8, cap: usize) -> i64 {
    let Some(h) = (unsafe { handle.as_ref() }) else {
        return -1;
    };
    let slice = unsafe { std::slice::from_raw_parts_mut(buf, cap) };
    match h.poll_into(slice) {
        Some(len) => len as i64,
        None => 0,
    }
}

/// The engine's sample counter (block-accurate, written by the audio
/// thread) — the sample clock with zero transport jitter.
///
/// # Safety
/// `handle` from [`clausters_open`].
#[cfg(feature = "realtime")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_clock(handle: *mut Clausters) -> u64 {
    match unsafe { handle.as_ref() } {
        Some(h) => h.segment.clock().load(Ordering::Acquire),
        None => 0,
    }
}

/// The device sample rate.
///
/// # Safety
/// `handle` from [`clausters_open`].
#[cfg(feature = "realtime")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_sample_rate(handle: *mut Clausters) -> f64 {
    match unsafe { handle.as_ref() } {
        Some(h) => h.segment.sample_rate(),
        None => 0.0,
    }
}

/// Writes a control bus directly in the data plane: the engine's `InCtl`
/// reads this very atomic on the next block — no command, no round trip.
///
/// # Safety
/// `handle` from [`clausters_open`].
#[cfg(feature = "realtime")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_ctl_set(handle: *mut Clausters, index: u32, value: f32) {
    if let Some(h) = unsafe { handle.as_ref() } {
        h.segment.control_buses().set(index as usize, value);
    }
}

/// Reads a control bus from the data plane.
///
/// # Safety
/// `handle` from [`clausters_open`].
#[cfg(feature = "realtime")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_ctl_get(handle: *mut Clausters, index: u32) -> f32 {
    match unsafe { handle.as_ref() } {
        Some(h) => h.segment.control_buses().get(index as usize),
        None => 0.0,
    }
}

/// Shuts the embedded server down (sends `/server_quit` through the ring, joins
/// the network thread, stops the audio stream) and frees the handle. The
/// shutdown is [`Clausters`]'s `Drop`; this just reclaims the box.
///
/// # Safety
/// `handle` from [`clausters_open`], used at most once.
#[cfg(feature = "realtime")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_close(handle: *mut Clausters) {
    if handle.is_null() {
        return;
    }
    // SAFETY: ownership returns from clausters_open's Box::into_raw; dropping
    // the box runs `Clausters::drop` (the /server_quit + join).
    drop(unsafe { Box::from_raw(handle) });
}

/// Reads an audio file into a malloc'd **interleaved** `f32` buffer — the
/// same decoder `/buffer_allocRead` uses, so a client never needs one of its own.
/// WAV goes through hound; FLAC, OGG/Vorbis, MP3, MP4/AAC, ALAC, AIFF and the
/// rest through symphonia. Integer files are scaled to `[-1, 1]`: whatever the
/// file holds, what comes back is `f32`.
///
/// Reads `num_frames` frames from `file_start` (`num_frames <= 0` means "to
/// the end"). Writes the frame count, channel count and the file's own sample
/// rate — the decoder never resamples — into the three out pointers. On
/// failure returns NULL and writes a message into (`err`, `err_cap`).
///
/// Free the result with [`clausters_free_samples`], passing
/// `frames * channels`.
///
/// # Safety
/// `path` must be a NUL-terminated UTF-8 string; the three out pointers must
/// be writable; `err` either NULL or writable for `err_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_read_soundfile(
    path: *const std::ffi::c_char,
    file_start: u64,
    num_frames: i64,
    out_frames: *mut u64,
    out_channels: *mut u32,
    out_sample_rate: *mut f64,
    err: *mut u8,
    err_cap: usize,
) -> *mut f32 {
    let result = (|| {
        if path.is_null() {
            return Err("null path".to_string());
        }
        // SAFETY: caller contract.
        let p = unsafe { std::ffi::CStr::from_ptr(path) }
            .to_str()
            .map_err(|e| format!("path is not UTF-8: {e}"))?;
        crate::server::nrt::read_audio(p, file_start as usize, num_frames)
    })();
    match result {
        Ok(buffer) => {
            // SAFETY: caller contract.
            unsafe {
                *out_frames = buffer.frames() as u64;
                *out_channels = buffer.channels() as u32;
                *out_sample_rate = buffer.sample_rate();
            }
            let mut samples = buffer.to_vec().into_boxed_slice();
            let ptr = samples.as_mut_ptr();
            std::mem::forget(samples);
            ptr
        }
        Err(e) => {
            write_error(&e, err, err_cap);
            std::ptr::null_mut()
        }
    }
}
