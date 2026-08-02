//! UDP OSC server implementing the M5 subset of the scsynth protocol:
//! `/server_status`, `/server_quit`, `/server_notify`, `/server_dumpOsc`, `/server_verbosity`, `/synth_new` (add actions 0-4),
//! `/node_free`, `/node_set`, `/node_before`, `/node_after`, `/group_new`, `/group_name`,
//! `/group_query`, `/group_freeAll`,
//! `/group_deepFree`, `/bus_set`, `/bus_get`, `/def_send synth`, `/def_free`; the buffer
//! commands `/buffer_alloc`, `/buffer_allocRead`, `/buffer_read`, `/buffer_write`, `/buffer_zero`,
//! `/buffer_free` (all async via the NRT thread, replying `/done cmd bufnum`),
//! `/buffer_query` (synchronous `/buffer_query.reply`), the synchronous reads `/buffer_get`
//! (`/buffer_get.reply`) and `/buffer_getRange` (`/buffer_getRange.reply`), and `/buffer_export` (dump raw samples to a
//! local file for the shared-resource bulk path); `/node_start` and
//! `/node_end` notifications go to `/server_notify` clients. With the `faust` feature,
//! `/def_send faust name def` compiles a def — JSON box graph (F2) or raw Faust
//! source (F1) — on the dedicated compiler thread and replies
//! `/done`/`/fail` asynchronously; `/synth_new` instantiates Faust defs like any
//! other (F3), with the def's UI parameters plus the reserved `out`/`in` bus
//! controls as `/node_set` names.
//!
//! This runs on the network thread: allocating and doing I/O here is fine.
//! It owns the [`EngineHandle`] and the SynthDef table: defs are compiled and
//! stored here, node commands are fully built here (boxed synth included) and
//! pushed to the engine's command FIFO; garbage coming back from the audio
//! thread is dropped here. Replies follow scsynth semantics (see the
//! `scsynth-osc` skill).

use std::io;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rosc::{OscBundle, OscMessage, OscPacket, OscTime, OscType, encoder};
use tracing::{error, info, warn};

use crate::dsp::ReplyKind;
#[cfg(feature = "faust")]
use crate::faust::compiler::{CacheJob, CompilePayload, CompileRequest, CompilerThread};
use crate::osc::ClientId;
use crate::osc::translate::{
    CmdTranslator, control_key, float_value, int_arg, parse_buffer_gen, parse_buffer_msg,
};
use crate::server::defstore::{self, DefKind, DefStore};
use crate::server::engine::{Cmd, EngineHandle, Garbage, NodeEventKind};
use crate::server::nrt::{NrtAction, NrtJob, NrtRequest, NrtRunner};

/// Default scsynth port.
pub const DEFAULT_PORT: u16 = 57110;

/// Largest UDP datagram we accept.
const RECV_BUF_SIZE: usize = 65536;

/// How long `recv_from` blocks before we take a garbage-collection pass.
const GC_INTERVAL: Duration = Duration::from_millis(100);

/// Fastest `/bus_stream` period a client can ask for (faster requests are
/// clamped, not failed): ~3x the interactive 30 Hz a GUI meter needs, and a
/// bound on how much reply traffic one client can subscribe to.
const MIN_STREAM_PERIOD: Duration = Duration::from_millis(10);

/// Most bus indices one `/bus_stream` subscription may list: 128 (index, value)
/// pairs fit comfortably in a single frame on every transport.
const MAX_STREAM_BUSES: usize = 128;

/// Most tap indices one `/bus_tapStream` subscription may list — one `/bus_tapStream.reply`
/// blob goes out per tap per period, so this bounds the reply traffic.
const MAX_STREAM_TAPS: usize = 8;

/// Largest `/bus_tapStream` window in samples for a **datagram-bounded** client
/// (UDP, and the 64 KiB IPC reply ring): a 32 KB blob (8192 × `f32`) leaves
/// room for the OSC envelope. A stream client (TCP/WebSocket) is bounded by
/// the configurable frame ceiling instead (M25). Every window is also clamped
/// to half the tap ring, the `tap_read_latest` tear-free bound.
const MAX_TAP_WINDOW: usize = 8192;

/// Information reported in `/server_status.reply` that does not come from the
/// engine counters.
pub struct ServerInfo {
    pub nominal_sample_rate: f64,
    pub actual_sample_rate: f64,
}

enum Flow {
    Continue,
    Quit,
}

pub struct OscServer {
    /// The UDP front. `None` for a headless pulled server ([`Self::headless`]):
    /// commands come only through the attached ring, replies only through it,
    /// and the host drives the loop by calling [`Self::step`].
    socket: Option<UdpSocket>,
    info: ServerInfo,
    handle: EngineHandle,
    /// Def tables, node→def mirror and message→command translation, shared
    /// with the NRT renderer (see [`crate::osc::translate`]).
    /// Owns the network-side buffer mirror (`translator.buffers`), updated
    /// when NRT results are installed: serves `/buffer_query` and gives `/buffer_read`,
    /// `/buffer_write` and `/buffer_zero` the current contents/shape, and a Faust
    /// instance its `soundfile` data.
    translator: CmdTranslator,
    nrt: NrtRunner,
    /// Clients registered via `/server_notify 1`; the client ID is index + 1.
    clients: Vec<ClientId>,
    /// Active `/bus_stream` subscriptions, at most one per client: the network
    /// counterpart of the shared-memory control-bus segment, for clients (a
    /// browser) that cannot map it. Pumped by the run loop.
    streams: Vec<BusStream>,
    /// Active `/bus_tapStream` subscriptions, at most one per client: the same
    /// network counterpart for the audio-tap rings. Pumped by the run loop.
    tap_streams: Vec<TapStream>,
    /// Which audio bus each tap ring is recording (`-1` = free), and how many
    /// watchers asked for it. **The server owns the rings**: a client names a
    /// bus and never an index, so this table is the whole of the bus -> ring
    /// assignment (its inverse is published in the segment for readers).
    tap_rings: Vec<i32>,
    tap_refs: Vec<u32>,
    /// Scratch window for tap snapshots, sized to the largest subscribed
    /// window; reused across pumps.
    tap_buf: Vec<f32>,
    recv_buf: Vec<u8>,
    /// Where streams and timetags read time from (see [`TimeSource`]).
    clock: TimeSource,
    /// M14: the shared-memory / in-process ring endpoint, when attached.
    ipc: Option<crate::server::ipc::IpcPeer>,
    /// TCP transport, when `listen_tcp` was called: accepts length-prefixed OSC
    /// connections multiplexed into the same loop. See [`crate::osc::tcp`].
    tcp: Option<crate::osc::tcp::TcpHub>,
    /// WebSocket transport, when `listen_ws` was called: the same OSC encoding
    /// over WebSocket binary messages, reachable from a browser. Multiplexed
    /// into the same loop as TCP. See [`crate::osc::ws`]. Native only: on
    /// wasm32 the engine lives in the page and is fed through the ring.
    #[cfg(not(target_arch = "wasm32"))]
    ws: Option<crate::osc::ws::WsHub>,
    /// M17 live MIDI input, when `listen_midi` was called: a virtual ALSA port
    /// whose decoded messages the loop drains. See [`crate::midi::live`].
    #[cfg(feature = "midi")]
    midi: Option<crate::midi::live::MidiHub>,
    /// On-disk def persistence, when a data directory is configured. Defs
    /// loaded from it on startup; `/def_send` write to it,
    /// `/def_free` deletes from it.
    store: Option<DefStore>,
    /// The compiler thread is owned here and dies with the server.
    #[cfg(feature = "faust")]
    faust_compiler: CompilerThread,
    /// `/server_sync` barrier bookkeeping. Each async pipeline (NRT buffers, Faust
    /// compiles) completes FIFO on its own thread, so a monotonic
    /// submitted/drained counter per pipeline is enough: a `/server_sync` records the
    /// current submitted counts as its targets and is answered with `/server_sync.reply`
    /// once both drained counts have caught up. See [`Self::handle_server_sync`].
    nrt_submitted: u64,
    nrt_drained: u64,
    faust_submitted: u64,
    faust_drained: u64,
    pending_syncs: Vec<PendingSync>,
    /// The shared beat grid (`/transport_set`), once a client defines one.
    transport: Option<Transport>,
    /// `/server_errorMode` mode: post command failures to the server console. The `/fail`
    /// OSC reply is always sent; this only gates the console logging. On by
    /// default (matches scsynth's default error-posting).
    post_errors: bool,
    /// Frame ceiling for the stream transports (TCP/WebSocket), in bytes
    /// (`--max-frame`, default [`crate::osc::DEFAULT_MAX_FRAME`]). Bounds what
    /// the hubs accept and what transport-aware replies (the `/bus_tapStream`
    /// window) may grow to; advertised in `/server_query.reply` so clients size
    /// their requests from it. UDP keeps the datagram cap regardless.
    max_frame: usize,
    /// Ceiling for concurrent stream clients, TCP + WebSocket combined
    /// (`--max-clients`, default [`crate::osc::DEFAULT_MAX_CLIENTS`]).
    max_clients: usize,
    /// The live-client slots both stream fronts share, created when the first
    /// of them binds (so `set_max_clients` can still change the ceiling).
    client_slots: Option<std::sync::Arc<crate::osc::ClientSlots>>,
}

/// The shared transport: a beat grid clients read to phase-align on the master
/// sample clock, plus a DAW-style **rolling state** (play / stop / position).
/// Beat `b` of the grid maps to sample `origin_sample + b·rate/tempo`; the
/// `playing` flag and `position` (the song-position beat where playback is or
/// will start) are the transport control a conductor sets and every client's
/// playhead obeys. The server only stores and **broadcasts** this (in-memory;
/// resets on restart) — it never schedules audio from it; each client rolls its
/// own playhead on the shared grid. See [`OscServer::handle_transport`].
#[derive(Clone, Copy)]
struct Transport {
    origin_sample: i64,
    tempo: f64,
    playing: bool,
    position: f64,
}

/// One client's `/bus_stream` subscription: which control buses it watches and
/// when its next `/bus_set` snapshot is due.
struct BusStream {
    client: ClientId,
    period: Duration,
    buses: Vec<i32>,
    /// In [`OscServer::mono_secs`] seconds (wall or sample time).
    next_due: f64,
}

/// One client's `/bus_tapStream` subscription: which audio taps it watches, the
/// window size of each `/bus_tapStream.reply` snapshot, and when the next one is due.
struct TapStream {
    client: ClientId,
    period: Duration,
    /// Snapshot window in samples (≤ [`MAX_TAP_WINDOW`], ≤ half the tap ring).
    frames: usize,
    /// The audio buses this subscription watches. It holds a watch on each for
    /// its lifetime, so a streaming client never issues `/bus_tap` itself.
    buses: Vec<i32>,
    /// In [`OscServer::mono_secs`] seconds (wall or sample time).
    next_due: f64,
}

/// A `/server_sync` waiting for the async pipelines to drain up to its targets.
struct PendingSync {
    client: ClientId,
    id: i32,
    nrt_target: u64,
    faust_target: u64,
}

/// Where the server reads time from. `Wall` is the native default: streams
/// pace on the monotonic clock and NTP timetags convert through the system
/// wall clock, as always. `Sample` is the headless/pulled mode (B1): both
/// derive from the **engine sample clock** — the only clock a wasm build has,
/// and the natural one for a host that drives `process_block` itself (an
/// offline host makes streams and timetags follow render time, not wall
/// time). `unix_epoch` anchors sample 0 on the Unix axis so wall-clocked
/// clients' timetags still land correctly.
enum TimeSource {
    /// Monotonic seconds since `epoch` (construction time).
    Wall { epoch: Instant },
    /// Seconds = engine sample clock / sample rate.
    Sample { unix_epoch: f64 },
}

impl OscServer {
    pub fn bind(
        addr: impl ToSocketAddrs,
        info: ServerInfo,
        handle: EngineHandle,
    ) -> io::Result<Self> {
        let socket = UdpSocket::bind(addr)?;
        // Periodic wakeups so garbage gets collected even without traffic.
        socket.set_read_timeout(Some(GC_INTERVAL))?;
        let translator = CmdTranslator::with_limits(
            handle.sample_rate,
            handle.audio_buses,
            handle.control_buses().len(),
            handle.limits,
        );
        Ok(Self {
            socket: Some(socket),
            info,
            handle,
            translator,
            nrt: NrtRunner::spawn(),
            clients: Vec::new(),
            streams: Vec::new(),
            tap_streams: Vec::new(),
            tap_rings: Vec::new(),
            tap_refs: Vec::new(),
            tap_buf: Vec::new(),
            clock: TimeSource::Wall {
                epoch: Instant::now(),
            },
            ipc: None,
            tcp: None,
            #[cfg(not(target_arch = "wasm32"))]
            ws: None,
            #[cfg(feature = "midi")]
            midi: None,
            store: None,
            recv_buf: vec![0; RECV_BUF_SIZE],
            #[cfg(feature = "faust")]
            faust_compiler: CompilerThread::spawn(),
            nrt_submitted: 0,
            nrt_drained: 0,
            faust_submitted: 0,
            faust_drained: 0,
            pending_syncs: Vec::new(),
            transport: None,
            post_errors: true,
            max_frame: crate::osc::DEFAULT_MAX_FRAME,
            max_clients: crate::osc::DEFAULT_MAX_CLIENTS,
            client_slots: None,
        })
    }

    /// A server with **no socket front** — the pulled mode (B1). Commands and
    /// replies travel only through the ring attached with
    /// [`Self::attach_ipc`], and the host drives the loop by calling
    /// [`Self::step`] before each block instead of [`Self::run`]. This is the
    /// shape behind the wasm/AudioWorklet build and the native pulled-callback
    /// embedding (`ClaustersHeadless`).
    ///
    /// Differences from [`Self::bind`], all consequences of having no thread
    /// of its own: NRT jobs run **inline** on the driving thread (same order,
    /// same results), and streams/timetags take their time from the **engine
    /// sample clock** rather than the wall clock (`TimeSource::Sample`) —
    /// `unix_epoch` (Unix seconds at sample 0) anchors that axis so a
    /// wall-clocked client's bundle timetags still land correctly; pass the
    /// current time for live use, or any fixed origin for deterministic runs.
    pub fn headless(info: ServerInfo, handle: EngineHandle, unix_epoch: f64) -> Self {
        let translator = CmdTranslator::with_limits(
            handle.sample_rate,
            handle.audio_buses,
            handle.control_buses().len(),
            handle.limits,
        );
        Self {
            socket: None,
            info,
            handle,
            translator,
            nrt: NrtRunner::inline(),
            clients: Vec::new(),
            streams: Vec::new(),
            tap_streams: Vec::new(),
            tap_rings: Vec::new(),
            tap_refs: Vec::new(),
            tap_buf: Vec::new(),
            clock: TimeSource::Sample { unix_epoch },
            ipc: None,
            tcp: None,
            #[cfg(not(target_arch = "wasm32"))]
            ws: None,
            #[cfg(feature = "midi")]
            midi: None,
            store: None,
            recv_buf: vec![0; RECV_BUF_SIZE],
            #[cfg(feature = "faust")]
            faust_compiler: CompilerThread::spawn(),
            nrt_submitted: 0,
            nrt_drained: 0,
            faust_submitted: 0,
            faust_drained: 0,
            pending_syncs: Vec::new(),
            transport: None,
            post_errors: true,
            max_frame: crate::osc::DEFAULT_MAX_FRAME,
            max_clients: crate::osc::DEFAULT_MAX_CLIENTS,
            client_slots: None,
        }
    }

    /// Monotonic seconds for stream pacing: wall time natively, engine sample
    /// time in the headless mode (so an offline drive paces streams in render
    /// time — deterministic, and the only clock wasm has).
    fn mono_secs(&self) -> f64 {
        match &self.clock {
            TimeSource::Wall { epoch } => epoch.elapsed().as_secs_f64(),
            TimeSource::Sample { .. } => {
                self.handle.current_samples() as f64 / self.handle.sample_rate as f64
            }
        }
    }

    /// Unix seconds for NTP timetag conversion (`/clock_query.reply`, bundle
    /// scheduling): the system wall clock natively; in the headless mode the
    /// sample axis anchored at `unix_epoch`, so the advertised clock anchor
    /// and incoming timetags stay mutually consistent.
    fn unix_secs(&self) -> f64 {
        match &self.clock {
            TimeSource::Wall { .. } => SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64(),
            TimeSource::Sample { unix_epoch } => {
                unix_epoch + self.handle.current_samples() as f64 / self.handle.sample_rate as f64
            }
        }
    }

    /// One pulled iteration of the serving loop: drain the ring, send due
    /// stream snapshots, collect garbage and async results. The headless
    /// counterpart of one [`Self::run`] turn — call it before each
    /// `process_block` (or at any convenient cadence). Returns `true` once a
    /// `/server_quit` has arrived.
    pub fn step(&mut self) -> bool {
        if let Flow::Quit = self.drain_ring() {
            return true;
        }
        self.pump_streams();
        self.pump_tap_streams();
        self.collect_garbage();
        self.collect_nrt_results();
        #[cfg(feature = "faust")]
        self.collect_faust_results();
        false
    }

    /// Sets the stream-transport frame ceiling (`--max-frame`), the largest
    /// OSC frame accepted from — and sent to — a TCP or WebSocket client.
    /// Clamped to at least the UDP receive buffer, so no transport ever
    /// carries less than a datagram. Call before [`Self::listen_tcp`] /
    /// [`Self::listen_ws`]: the hubs capture the ceiling when they bind.
    pub fn set_max_frame(&mut self, bytes: usize) {
        self.max_frame = bytes.max(RECV_BUF_SIZE);
    }

    /// Sets the ceiling for concurrent stream clients, TCP + WebSocket
    /// combined (`--max-clients`); a connection past it is dropped at accept.
    /// Call before [`Self::listen_tcp`] / [`Self::listen_ws`]: the shared
    /// slot pool is created when the first front binds.
    pub fn set_max_clients(&mut self, n: usize) {
        self.max_clients = n.max(1);
    }

    /// The live-client slots both stream fronts share, created on first use.
    fn client_slots(&mut self) -> std::sync::Arc<crate::osc::ClientSlots> {
        let max = self.max_clients;
        self.client_slots
            .get_or_insert_with(|| std::sync::Arc::new(crate::osc::ClientSlots::new(max)))
            .clone()
    }

    /// Enables on-disk persistence and reloads whatever defs the store
    /// already holds. SynthDefs are recompiled inline (cheap); Faust defs are
    /// queued on the compiler thread, restoring from the bitcode cache when
    /// possible, so the socket starts serving immediately and the library
    /// loads incrementally.
    pub fn attach_store(&mut self, store: DefStore) {
        #[cfg(feature = "synth")]
        for spec in store.load_synthdef_specs() {
            if let Err(e) = self.translator.d_recv(&[OscType::Blob(spec)]) {
                warn!("persisted SynthDef failed to load: {e}");
            }
        }
        // Same courtesy as the Faust case below: a build without the `synth`
        // family cannot reload persisted SynthDefs, so say it once.
        #[cfg(not(feature = "synth"))]
        if std::fs::read_dir(store.synthdefs_dir())
            .map(|mut entries| entries.any(|e| e.is_ok()))
            .unwrap_or(false)
        {
            warn!(
                "persisted SynthDefs found but this build lacks the `synth` feature; skipping them"
            );
        }
        #[cfg(feature = "faust")]
        for record in crate::faust::cache::load_records(store.faustdefs_dir()) {
            let request = CompileRequest {
                name: record.name.clone(),
                payload: record.to_payload(),
                client: None,
                cache: Some(Box::new(CacheJob {
                    dir: store.faustdefs_dir().to_path_buf(),
                    restore: Some(record),
                })),
            };
            if self.faust_compiler.submit(request).is_err() {
                warn!("compiler thread down: cannot reload persisted Faust defs");
                break;
            }
            self.faust_submitted += 1;
        }
        // Without the `faust` feature there is no compiler to reload them, so a
        // bundle's Faust defs are silently inert — warn so a standalone built
        // without `faust` does not look like it "lost" instruments.
        #[cfg(not(feature = "faust"))]
        if std::fs::read_dir(store.faustdefs_dir())
            .map(|mut entries| entries.any(|e| e.is_ok()))
            .unwrap_or(false)
        {
            warn!(
                "persisted Faust defs found but this build lacks the `faust` feature; skipping them"
            );
        }
        // GraphDefs load after the synth/faust defs (their members may
        // reference those names); validation is structural, so any still-
        // missing member only fails later at /graph_new (M18).
        for spec in store.load_graphdef_specs() {
            if let Err(e) = self.translator.d_graph(&[OscType::Blob(spec)]) {
                warn!("persisted GraphDef failed to load: {e}");
            }
        }
        // M19 boot order: defs -> graphdefs -> bindings -> boot preset, so a
        // binding's instrument and a boot graph's name already resolve.
        for pb in store.load_bindings() {
            let channel = pb.channel;
            let mut cmds = Vec::new();
            match self.translator.restore_binding(pb, &mut cmds) {
                Ok(()) => self.ship_boot_cmds(cmds),
                Err(e) => warn!("persisted MIDI binding (channel {channel}) failed: {e}"),
            }
        }
        for boot in store.load_boot() {
            let mut args = vec![
                OscType::String(boot.graph.clone()),
                OscType::Int(-1),
                OscType::Int(0),
                OscType::Int(0),
            ];
            for (port, value) in boot.ports {
                args.push(OscType::String(port));
                args.push(OscType::Float(value));
            }
            let msg = OscMessage {
                addr: "/graph_new".into(),
                args,
            };
            let mut cmds = Vec::new();
            match self.translator.translate(&msg, &mut cmds) {
                Ok(()) => self.ship_boot_cmds(cmds),
                Err(e) => warn!("boot graph '{}' failed: {e}", boot.graph),
            }
        }
        self.store = Some(store);
    }

    /// Ships commands produced during the boot reload (binding restore / boot
    /// preset) to the engine. A full FIFO at boot is logged, not fatal.
    fn ship_boot_cmds(&mut self, cmds: Vec<Cmd>) {
        for cmd in cmds {
            if self.handle.send(cmd).is_err() {
                warn!("command FIFO full during boot reload");
                break;
            }
        }
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.udp()?.local_addr()
    }

    /// The UDP socket, or the error a socket-front operation reports on a
    /// headless server.
    fn udp(&self) -> io::Result<&UdpSocket> {
        self.socket
            .as_ref()
            .ok_or_else(|| io::Error::other("headless server: no UDP front"))
    }

    /// Starts accepting length-prefixed OSC over TCP on `addr` (server track M /
    /// client C8). The run loop drains the connections every iteration and a
    /// zero-length UDP datagram to our own address wakes it the moment a frame
    /// arrives, so TCP requests don't wait for the GC tick. Returns the bound
    /// TCP address.
    pub fn listen_tcp(&mut self, addr: impl ToSocketAddrs) -> io::Result<SocketAddr> {
        // Reader threads wake the loop by pinging the UDP socket; if we bound to
        // an unspecified address, ping loopback on the same port.
        let mut wake_target = self.udp()?.local_addr()?;
        if wake_target.ip().is_unspecified() {
            wake_target.set_ip(match wake_target {
                SocketAddr::V4(_) => std::net::Ipv4Addr::LOCALHOST.into(),
                SocketAddr::V6(_) => std::net::Ipv6Addr::LOCALHOST.into(),
            });
        }
        let slots = self.client_slots();
        let hub = crate::osc::tcp::TcpHub::bind(addr, wake_target, self.max_frame, slots)?;
        let bound = hub.local_addr();
        self.tcp = Some(hub);
        Ok(bound)
    }

    /// Starts accepting OSC over WebSocket on `addr`. Same loop multiplexing and
    /// zero-length-UDP wake as [`Self::listen_tcp`]: the run loop drains
    /// WebSocket frames every iteration and a connection thread pings our UDP
    /// socket the moment a frame arrives. Returns the bound address; connect a
    /// browser with `ws://<addr>/`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn listen_ws(&mut self, addr: impl ToSocketAddrs) -> io::Result<SocketAddr> {
        let mut wake_target = self.udp()?.local_addr()?;
        if wake_target.ip().is_unspecified() {
            wake_target.set_ip(match wake_target {
                SocketAddr::V4(_) => std::net::Ipv4Addr::LOCALHOST.into(),
                SocketAddr::V6(_) => std::net::Ipv6Addr::LOCALHOST.into(),
            });
        }
        let slots = self.client_slots();
        let hub = crate::osc::ws::WsHub::bind(addr, wake_target, self.max_frame, slots)?;
        let bound = hub.local_addr();
        self.ws = Some(hub);
        Ok(bound)
    }

    /// M17: opens a virtual MIDI input port named `port_name`. The `midir`
    /// input thread wakes the loop with a zero-length UDP datagram (same
    /// mechanism as TCP), so MIDI messages are served without waiting for the
    /// GC tick. See [`crate::midi::live`].
    #[cfg(feature = "midi")]
    pub fn listen_midi(&mut self, port_name: &str) -> io::Result<()> {
        let mut wake_target = self.udp()?.local_addr()?;
        if wake_target.ip().is_unspecified() {
            wake_target.set_ip(match wake_target {
                SocketAddr::V4(_) => std::net::Ipv4Addr::LOCALHOST.into(),
                SocketAddr::V6(_) => std::net::Ipv6Addr::LOCALHOST.into(),
            });
        }
        let hub =
            crate::midi::live::MidiHub::open(port_name, wake_target).map_err(io::Error::other)?;
        self.midi = Some(hub);
        Ok(())
    }

    /// M14: attaches the ring endpoint of an IPC segment. The run loop then
    /// drains it on every iteration; to keep ring latency low without a
    /// cross-process semaphore (v1 trade-off), the socket timeout — the
    /// loop's tick — is shortened.
    pub fn attach_ipc(&mut self, peer: crate::server::ipc::IpcPeer) -> io::Result<()> {
        if let Some(socket) = &self.socket {
            socket.set_read_timeout(Some(Duration::from_millis(2)))?;
        }
        self.ipc = Some(peer);
        Ok(())
    }

    /// Blocks serving requests until a `/server_quit` arrives. Requires the UDP
    /// front ([`Self::bind`]); a headless server is driven by [`Self::step`].
    pub fn run(&mut self) -> io::Result<()> {
        self.udp()?;
        loop {
            if let Flow::Quit = self.drain_ring() {
                return Ok(());
            }
            if let Flow::Quit = self.drain_tcp() {
                return Ok(());
            }
            if let Flow::Quit = self.drain_ws() {
                return Ok(());
            }
            self.drain_midi();
            self.prune_disconnected();
            self.pump_streams();
            self.pump_tap_streams();
            let socket = self.socket.as_ref().expect("run() checked the socket");
            let (len, from) = match socket.recv_from(&mut self.recv_buf) {
                Ok(ok) => ok,
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    self.collect_garbage();
                    self.collect_nrt_results();
                    #[cfg(feature = "faust")]
                    self.collect_faust_results();
                    continue;
                }
                // A previous send to a now-closed client port can surface as
                // ECONNREFUSED on the next recv (Linux); not fatal.
                Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => continue,
                // A signal delivered to this thread interrupts the recv with
                // EINTR (SA_RESTART does not restart a recv under a socket
                // timeout); a signal is not an error, keep serving.
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            if len == 0 {
                // A zero-length datagram is a TCP wake (a reader queued a frame
                // or a disconnect): loop back to drain TCP/ring promptly.
                continue;
            }
            // The single decode entry point for every transport (`crate::osc`).
            let packet = match crate::osc::decode_packet(&self.recv_buf[..len]) {
                Ok(packet) => packet,
                Err(e) => {
                    warn!("malformed OSC packet from {from}: {e}");
                    continue;
                }
            };
            let flow = self.handle_packet(packet, ClientId::Udp(from));
            self.collect_garbage();
            self.collect_nrt_results();
            #[cfg(feature = "faust")]
            self.collect_faust_results();
            if let Flow::Quit = flow {
                return Ok(());
            }
        }
    }

    /// M14: handles every packet waiting in the attached ring. Same
    /// validation path as UDP (`decode_packet`); ring bytes are untrusted.
    fn drain_ring(&mut self) -> Flow {
        if self.ipc.is_none() {
            return Flow::Continue;
        }
        loop {
            let Some(ipc) = &self.ipc else { unreachable!() };
            let mut buf = std::mem::take(&mut self.recv_buf);
            let popped = ipc.try_pop(&mut buf);
            self.recv_buf = buf;
            let Some(len) = popped else {
                return Flow::Continue;
            };
            let packet = match crate::osc::decode_packet(&self.recv_buf[..len]) {
                Ok(packet) => packet,
                Err(e) => {
                    warn!("malformed OSC packet from ring client: {e}");
                    continue;
                }
            };
            if let Flow::Quit = self.handle_packet(packet, ClientId::Ring) {
                return Flow::Quit;
            }
        }
    }

    /// Handles every complete TCP frame currently queued. Same validation path
    /// as UDP (`decode_packet`); TCP bytes are untrusted. Replies route back to
    /// the originating connection via [`ClientId::Tcp`].
    fn drain_tcp(&mut self) -> Flow {
        loop {
            // Scope the `&mut self.tcp` borrow so `handle_packet(&mut self)` and
            // its replies (which read `self.tcp`) can run.
            let next = match &mut self.tcp {
                Some(hub) => hub.next_frame(),
                None => return Flow::Continue,
            };
            let Some((id, bytes)) = next else {
                return Flow::Continue;
            };
            let packet = match crate::osc::decode_packet(&bytes) {
                Ok(packet) => packet,
                Err(e) => {
                    warn!("malformed OSC packet from tcp client {id}: {e}");
                    continue;
                }
            };
            let flow = self.handle_packet(packet, ClientId::Tcp(id));
            self.collect_garbage();
            self.collect_nrt_results();
            #[cfg(feature = "faust")]
            self.collect_faust_results();
            if let Flow::Quit = flow {
                return Flow::Quit;
            }
        }
    }

    /// No WebSocket hub exists on wasm32; the stub keeps the run loop's shape.
    #[cfg(target_arch = "wasm32")]
    fn drain_ws(&mut self) -> Flow {
        Flow::Continue
    }

    /// Handles every complete WebSocket frame currently queued. Same validation
    /// path as UDP (`decode_packet`); WebSocket bytes are untrusted. Replies
    /// route back to the originating connection via [`ClientId::Ws`].
    #[cfg(not(target_arch = "wasm32"))]
    fn drain_ws(&mut self) -> Flow {
        loop {
            // Scope the `&mut self.ws` borrow so `handle_packet(&mut self)` and
            // its replies (which read `self.ws`) can run.
            let next = match &mut self.ws {
                Some(hub) => hub.next_frame(),
                None => return Flow::Continue,
            };
            let Some((id, bytes)) = next else {
                return Flow::Continue;
            };
            let packet = match crate::osc::decode_packet(&bytes) {
                Ok(packet) => packet,
                Err(e) => {
                    warn!("malformed OSC packet from ws client {id}: {e}");
                    continue;
                }
            };
            let flow = self.handle_packet(packet, ClientId::Ws(id));
            self.collect_garbage();
            self.collect_nrt_results();
            #[cfg(feature = "faust")]
            self.collect_faust_results();
            if let Flow::Quit = flow {
                return Flow::Quit;
            }
        }
    }

    /// M17: translates every queued live-MIDI message into engine commands and
    /// ships them. Each message is self-contained (one note/control event), so
    /// it is realized like the immediate OSC forms: `translate_midi` (which
    /// reuses the `/synth_new`/`/node_set`/`/node_free` path and keeps the tree mirror in
    /// sync), then ship the batch. MIDI never quits the server.
    #[cfg(feature = "midi")]
    fn drain_midi(&mut self) {
        let mut cmds = Vec::new();
        while let Some(msg) = self.midi.as_ref().and_then(|hub| hub.try_next()) {
            cmds.clear();
            if let Err(e) = self.translator.translate_midi(msg, &mut cmds) {
                warn!("midi: {e}");
                continue;
            }
            for cmd in cmds.drain(..) {
                if self.handle.send(cmd).is_err() {
                    warn!("midi: command FIFO full");
                    break;
                }
            }
        }
        self.collect_garbage();
    }

    #[cfg(not(feature = "midi"))]
    fn drain_midi(&mut self) {}

    /// Drains finished compilations: stores factories and sends the async
    /// `/done`/`/fail` replies. Called from the same places as
    /// `collect_garbage` (after each packet and on the GC tick).
    #[cfg(feature = "faust")]
    fn collect_faust_results(&mut self) {
        while let Some(result) = self.faust_compiler.try_result() {
            self.faust_drained += 1;
            match result.outcome {
                Ok(def) => {
                    self.translator
                        .faust_defs
                        .insert(result.name.clone(), Arc::new(def));
                    // No client on a startup reload: nothing to answer.
                    if let Some(client) = result.client {
                        self.reply(
                            client,
                            "/done",
                            vec![
                                OscType::String("/def_send".into()),
                                OscType::String("faust".into()),
                                OscType::String(result.name),
                            ],
                        );
                    }
                }
                Err(error) => match result.client {
                    Some(client) => self.fail(client, "/def_send", error),
                    None => warn!("persisted Faust def '{}' failed: {error}", result.name),
                },
            }
        }
        self.resolve_syncs();
    }

    /// `/def_send faust <name> <def>`: queue an async Faust compilation. The def
    /// format is sniffed by [`CompilePayload::classify`]: raw Faust source (F1),
    /// a JSON box graph (F2, `faust::boxes`), or a JSON signal tree
    /// (`faust::signals`, root `{"signals": …}`).
    #[cfg(feature = "faust")]
    fn handle_def_send_faust(&mut self, args: &[OscType], from: ClientId) {
        let (name, def) = match crate::osc::translate::parse_def_send_faust(args) {
            Ok(pair) => pair,
            Err(e) => return self.fail(from, "/def_send", e),
        };
        let payload = CompilePayload::classify(def);
        self.claim_def_name(&name, DefKind::Faust);
        // A live faust /def_send always compiles fresh from the given def and, with
        // persistence on, (re)writes the cache (restore = None). An ephemeral
        // def never reaches the store: its bitcode speed-cache goes to the OS
        // temp directory instead, so replaying the same expression still skips
        // the recompile without leaving a record behind.
        let cache = if defstore::is_ephemeral(&name) {
            let dir = defstore::ephemeral_dir();
            std::fs::create_dir_all(&dir)
                .is_ok()
                .then(|| Box::new(CacheJob { dir, restore: None }))
        } else {
            self.store.as_ref().map(|s| {
                Box::new(CacheJob {
                    dir: s.faustdefs_dir().to_path_buf(),
                    restore: None,
                })
            })
        };
        let request = CompileRequest {
            name,
            payload,
            client: Some(from),
            cache,
        };
        if self.faust_compiler.submit(request).is_err() {
            self.fail(from, "/def_send", "compiler thread is down");
        } else {
            self.faust_submitted += 1;
        }
    }

    #[cfg(not(feature = "faust"))]
    fn handle_def_send_faust(&mut self, _args: &[OscType], from: ClientId) {
        self.fail(from, "/def_send", "server built without faust support");
    }

    /// `/server_sync id`: the async barrier (scsynth semantics). Records the current
    /// submitted counts as targets and is answered with `/server_sync.reply id` once both
    /// async pipelines (NRT buffers, Faust compiles) have drained up to them —
    /// i.e. every async command received before this `/server_sync` has finished.
    /// Each pipeline completes FIFO, so the counters are a sufficient barrier.
    fn handle_server_sync(&mut self, msg: &OscMessage, from: ClientId) {
        let id = match msg.args.first() {
            Some(OscType::Int(id)) => *id,
            _ => return self.fail(from, "/server_sync", "expected an int id"),
        };
        self.pending_syncs.push(PendingSync {
            client: from,
            id,
            nrt_target: self.nrt_submitted,
            faust_target: self.faust_submitted,
        });
        self.resolve_syncs(); // answer at once if nothing is outstanding
    }

    /// Answers every pending `/server_sync` whose target counts have been reached.
    /// Called after each async drain (and from [`Self::handle_server_sync`]).
    fn resolve_syncs(&mut self) {
        if self.pending_syncs.is_empty() {
            return;
        }
        let (nrt, faust) = (self.nrt_drained, self.faust_drained);
        let mut ready = Vec::new();
        self.pending_syncs.retain(|p| {
            let done = nrt >= p.nrt_target && faust >= p.faust_target;
            if done {
                ready.push((p.client, p.id));
            }
            !done
        });
        for (client, id) in ready {
            self.reply(client, "/server_sync.reply", vec![OscType::Int(id)]);
        }
    }

    /// Drops what the audio thread discarded, keeps the def mirror in sync
    /// and forwards node lifecycle events to `/server_notify` clients.
    fn collect_garbage(&mut self) {
        while let Some(g) = self.handle.pop_garbage() {
            match g {
                Garbage::FreedSynth { id, .. } => {
                    self.translator.forget_node(id);
                }
                Garbage::FreedGroup { .. } | Garbage::FreedBuffer(_) => {}
                Garbage::SpentBundle(cmds) => {
                    // Empty: the executed shell of a timed bundle. Non-empty:
                    // the engine's schedule queue was full.
                    if !cmds.is_empty() {
                        warn!("engine rejected a timed bundle (schedule queue full)");
                    }
                }
                Garbage::RejectedSynth { id, .. } | Garbage::RejectedGroup { id, .. } => {
                    // Don't touch the mirror: on a duplicate-ID rejection the
                    // original node is still alive under this ID. The rejected
                    // id never became a node — return it to its registry, and
                    // tell the `/server_notify` clients (the rejection is async, so
                    // there is no requester to reply to): a client registry
                    // reconciles its in-flight id off this `/fail`, since no
                    // `/node_end` will ever come for it.
                    self.translator.release_node_id(id);
                    warn!("engine rejected node {id} (duplicate ID, bad target or full table)");
                    let args = vec![
                        OscType::String("/synth_new".into()),
                        OscType::String(format!(
                            "engine rejected node {id}: duplicate ID, bad target or full table"
                        )),
                        OscType::Int(id),
                    ];
                    for client in &self.clients {
                        self.reply(*client, "/fail", args.clone());
                    }
                }
            }
        }
        while let Some(ev) = self.handle.pop_event() {
            let addr = match ev.kind {
                NodeEventKind::Go => "/node_start",
                NodeEventKind::End => "/node_end",
            };
            // A death returns the id to whichever server-owned range it came
            // from (auto, MIDI); client-range ids recycle client-side off the
            // same `/node_end` broadcast below.
            if ev.kind == NodeEventKind::End {
                self.translator.release_node_id(ev.id);
            }
            // id, parent, previous, next, isGroup, name. We don't track
            // sibling IDs on this side, so previous/next are -1. The name is
            // the group's `/group_name` (empty for a synth or an unnamed
            // group): a client watching the tree learns *which* channel came
            // up or went away without a follow-up query — and for a death
            // there is no query left to make, which is why the mirror keeps
            // the label one beat longer than the entry.
            let name = match (ev.is_group, ev.kind) {
                (false, _) => String::new(),
                (true, NodeEventKind::Go) => self.translator.group_name(ev.id).to_string(),
                (true, NodeEventKind::End) => self.translator.take_group_epitaph(ev.id),
            };
            let args = vec![
                OscType::Int(ev.id),
                OscType::Int(ev.parent_id),
                OscType::Int(-1),
                OscType::Int(-1),
                OscType::Int(ev.is_group as i32),
                OscType::String(name),
            ];
            for client in &self.clients {
                self.reply(*client, addr, args.clone());
            }
        }
        // Side-effect replies (S9): `SendTrig`/`SendReply` reply to `/server_notify`
        // clients; `Poll` posts to the server console and, when its trigid is
        // set, also sends `/node_trigger`.
        while let Some(msg) = self.handle.pop_reply() {
            match msg.kind {
                ReplyKind::Trig => {
                    let value = msg.values().first().copied().unwrap_or(0.0);
                    self.notify_trigger(msg.node_id, msg.id, value);
                }
                ReplyKind::Reply => {
                    // Custom address `cmdName nodeID replyID value…`.
                    let mut args = vec![OscType::Int(msg.node_id), OscType::Int(msg.id)];
                    args.extend(msg.values().iter().map(|v| OscType::Float(*v)));
                    let addr = msg.name().to_string();
                    for client in &self.clients {
                        self.reply(*client, &addr, args.clone());
                    }
                }
                ReplyKind::Poll => {
                    let value = msg.values().first().copied().unwrap_or(0.0);
                    info!(target: crate::logging::OSC_TARGET, "{}: {value}", msg.name());
                    if msg.id >= 0 {
                        self.notify_trigger(msg.node_id, msg.id, value);
                    }
                }
            }
        }
    }

    /// Sends `/node_trigger nodeID triggerID value` to every `/server_notify` client (the shape
    /// `SendTrig` and a `Poll` with a trigid produce).
    fn notify_trigger(&self, node_id: i32, trig_id: i32, value: f32) {
        let args = vec![
            OscType::Int(node_id),
            OscType::Int(trig_id),
            OscType::Float(value),
        ];
        for client in &self.clients {
            self.reply(*client, "/node_trigger", args.clone());
        }
    }

    fn handle_packet(&mut self, packet: OscPacket, from: ClientId) -> Flow {
        match packet {
            OscPacket::Message(msg) => self.handle_message(msg, from),
            OscPacket::Bundle(bundle) => self.handle_bundle(bundle, from),
        }
    }

    /// Bundles with the "immediately" timetag (or a past one — scsynth also
    /// runs late bundles right away) execute now; future timetags are
    /// converted to a sample target and shipped to the engine's scheduler,
    /// which fires them sample-accurately (M6).
    fn handle_bundle(&mut self, bundle: OscBundle, from: ClientId) -> Flow {
        match self.timetag_delta_secs(bundle.timetag) {
            Some(delta) if delta > 0.0 => {
                self.schedule_bundle(bundle, delta, from);
                Flow::Continue
            }
            Some(delta) => {
                warn!("late bundle ({:.3}s): executing immediately", -delta);
                self.run_bundle_now(bundle, from)
            }
            None => self.run_bundle_now(bundle, from),
        }
    }

    fn run_bundle_now(&mut self, bundle: OscBundle, from: ClientId) -> Flow {
        for packet in bundle.content {
            if let Flow::Quit = self.handle_packet(packet, from) {
                return Flow::Quit;
            }
        }
        Flow::Continue
    }

    /// Builds every message of a timed bundle into engine commands (synths
    /// boxed, names resolved — all the allocating work happens now) and
    /// sends them as one atomic [`Cmd::Schedule`].
    fn schedule_bundle(&mut self, bundle: OscBundle, delta: f64, from: ClientId) {
        let time = self.handle.current_samples() + (delta * self.handle.sample_rate as f64) as u64;
        let mut cmds = Vec::new();
        for packet in bundle.content {
            match packet {
                OscPacket::Message(msg) => {
                    tracing::trace!(target: crate::logging::OSC_TARGET, "{} {:?} (in {delta:.3}s)", msg.addr, msg.args);
                    if let Err(e) = self.schedule_message(&msg, &mut cmds) {
                        self.fail(from, &msg.addr, e);
                    }
                }
                // A nested bundle carries its own timetag: scheduled (or
                // executed) independently.
                OscPacket::Bundle(inner) => {
                    self.handle_bundle(inner, from);
                }
            }
        }
        if !cmds.is_empty() && self.handle.send(Cmd::Schedule { time, cmds }).is_err() {
            self.fail(from, "#bundle", "command FIFO full");
        }
    }

    /// Translates one schedulable message into commands (shared with the NRT
    /// renderer). Nothing reaches the engine until the whole bundle is
    /// assembled.
    fn schedule_message(&mut self, msg: &OscMessage, cmds: &mut Vec<Cmd>) -> Result<(), String> {
        self.translator.translate(msg, cmds)
    }

    fn handle_message(&mut self, msg: OscMessage, from: ClientId) -> Flow {
        tracing::trace!(target: crate::logging::OSC_TARGET, "{} {:?}", msg.addr, msg.args);
        match msg.addr.as_str() {
            "/server_status" => self.send_server_status(from),
            "/server_query" => self.send_server_query(from),
            "/server_notify" => self.handle_server_notify(&msg, from),
            // The translator covers the whole schedulable subset (and keeps
            // the M12 tree mirror in sync), so the immediate forms share one
            // path: translate, then ship every command.
            "/synth_new"
            | "/group_new"
            | "/group_freeAll"
            | "/group_deepFree"
            | "/node_free"
            | "/node_run"
            | "/node_set"
            | "/node_setRange"
            | "/node_fill"
            | "/node_map"
            | "/node_mapAudio"
            | "/node_mapRange"
            | "/node_mapAudioRange"
            | "/node_before"
            | "/node_after"
            | "/node_order"
            | "/group_head"
            | "/group_tail"
            | "/group_sortMode"
            | "/group_parallel"
            | "/group_name"
            | "/graph_new"
            | "/graph_newVoice" => self.handle_via_translate(&msg, from),
            "/node_trace" => self.handle_node_trace(&msg, from),
            // MIDI binding mutations also persist the binding set (M19).
            "/midi_bind" | "/midi_unbind" | "/midi_map" => {
                self.handle_via_translate(&msg, from);
                self.persist_bindings();
            }
            "/group_queryTree" => self.handle_group_query_tree(&msg, from),
            "/group_query" => self.handle_group_query(&msg, from),
            "/node_query" => self.handle_node_query(&msg, from),
            "/group_dumpGraph" => self.handle_group_dump_graph(&msg, from),
            "/bus_set" => self.handle_bus_set(&msg, from),
            "/bus_get" => self.handle_bus_get(&msg, from),
            "/bus_setRange" => self.handle_bus_set_range(&msg, from),
            "/bus_getRange" => self.handle_bus_get_range(&msg, from),
            "/bus_fill" => self.handle_bus_fill(&msg, from),
            "/bus_stream" => self.handle_bus_stream(&msg, from),
            "/bus_tap" => self.handle_bus_tap(&msg, from),
            "/bus_tapStream" => self.handle_bus_tap_stream(&msg, from),
            "/synth_get" => self.handle_synth_get(&msg, from, false),
            "/synth_getRange" => self.handle_synth_get(&msg, from, true),
            "/synth_forgetId" => self.handle_synth_forget_id(&msg, from),
            "/buffer_close" => self.handle_buffer_close(&msg, from),
            "/def_load" => self.handle_def_load(&msg, from),
            "/def_loadDir" => self.handle_def_load_dir(&msg, from),
            "/sched_clear" => self.handle_sched_clear(from),
            "/server_errorMode" => self.handle_server_error_mode(&msg, from),
            "/server_cmd" => self.handle_server_cmd(&msg, from),
            "/node_ugenCmd" => self.handle_via_translate(&msg, from),
            "/clock_query" => self.handle_clock_query(from),
            "/sched_at" => self.handle_sched_at(&msg, from),
            "/buffer_alloc" => self.handle_buffer_cmd(&msg, from, "/buffer_alloc"),
            "/buffer_allocRead" => self.handle_buffer_cmd(&msg, from, "/buffer_allocRead"),
            "/buffer_read" => self.handle_buffer_cmd(&msg, from, "/buffer_read"),
            "/buffer_write" => self.handle_buffer_cmd(&msg, from, "/buffer_write"),
            "/buffer_zero" => self.handle_buffer_cmd(&msg, from, "/buffer_zero"),
            "/buffer_gen" => self.handle_buffer_gen(&msg, from),
            "/buffer_free" => self.handle_buffer_cmd(&msg, from, "/buffer_free"),
            "/buffer_query" => self.handle_buffer_query(&msg, from),
            "/def_query" => self.handle_def_query(&msg, from),
            "/ugen_query" => self.handle_ugen_query(&msg, from),
            "/buffer_get" => self.handle_buffer_get(&msg, from),
            "/buffer_getRange" => self.handle_buffer_get_range(&msg, from),
            "/buffer_export" => self.handle_buffer_export(&msg, from),
            "/server_sync" => self.handle_server_sync(&msg, from),
            "/def_send" => self.handle_def_send(&msg, from),
            "/def_free" => self.handle_def_free(&msg, from),
            "/server_dumpOsc" => self.handle_server_dump_osc(&msg, from),
            "/server_verbosity" => self.handle_server_verbosity(&msg, from),
            "/transport_query" => self.handle_transport_query(from),
            "/transport_set" => self.handle_transport(&msg, from),
            "/transport_play" => self.handle_transport_play(&msg, from),
            "/transport_stop" => self.handle_transport_stop(from),
            "/transport_locate" => self.handle_transport_locate(&msg, from),
            "/server_quit" => {
                self.reply(from, "/done", vec![OscType::String("/server_quit".into())]);
                return Flow::Quit;
            }
            other => self.fail(from, other, "unknown command"),
        }
        Flow::Continue
    }

    /// `/server_dumpOsc flag`: toggles the OSC-traffic log overlay (the `clausters::osc`
    /// trace target). Unlike scsynth's console dump, this routes through the
    /// logging system the client also controls with `/server_verbosity`; output is on
    /// the server's stderr. Replies `/done`.
    fn handle_server_dump_osc(&mut self, msg: &OscMessage, from: ClientId) {
        let on = matches!(msg.args.first(), Some(OscType::Int(n)) if *n != 0);
        match crate::logging::set_osc_dump(on) {
            Ok(()) => self.reply(
                from,
                "/done",
                vec![OscType::String("/server_dumpOsc".into())],
            ),
            Err(e) => self.fail(from, "/server_dumpOsc", e),
        }
    }

    /// `/server_verbosity level`: the client retunes the server's log level live.
    /// `level` is an int (`-1` errors, `0` warn, `1` info, `2` debug, `3+`
    /// trace) or a string `EnvFilter` directive (e.g. `"clausters::osc=trace"`).
    /// Replies `/done`. (Uncommon, but it lets a client steer server logs
    /// without restarting; the initial level comes from `-v`/`RUST_LOG`.)
    fn handle_server_verbosity(&mut self, msg: &OscMessage, from: ClientId) {
        let result = match msg.args.first() {
            Some(OscType::Int(n)) => crate::logging::set_verbosity(*n as i8),
            Some(OscType::String(s)) => crate::logging::set_base(s),
            _ => Err("expected an int level or a string filter directive".to_string()),
        };
        match result {
            Ok(()) => self.reply(
                from,
                "/done",
                vec![OscType::String("/server_verbosity".into())],
            ),
            Err(e) => self.fail(from, "/server_verbosity", e),
        }
    }

    fn send_server_status(&mut self, to: ClientId) {
        let counters = self.handle.counters();
        let num_defs = self.translator.def_count();
        // avg/peak CPU are the engine's per-block load as a *percentage* of
        // the block budget (scsynth's convention). Peak is per poll window:
        // reading it resets it. The trailing int (late blocks since boot, our
        // engine-side xrun proxy) is appended after the scsynth-shaped fields,
        // so positional readers keep working.
        let args = vec![
            OscType::Int(1),
            OscType::Int(counters.ugens.load(Ordering::Relaxed) as i32),
            OscType::Int(counters.synths.load(Ordering::Relaxed) as i32),
            OscType::Int(counters.groups.load(Ordering::Relaxed) as i32),
            OscType::Int(num_defs as i32),
            OscType::Float(counters.avg_cpu() * 100.0),
            OscType::Float(counters.take_peak_cpu() * 100.0),
            OscType::Double(self.info.nominal_sample_rate),
            OscType::Double(self.info.actual_sample_rate),
            OscType::Int(counters.late_blocks() as i32),
        ];
        self.reply(to, "/server_status.reply", args);
    }

    /// Reports the server's static configuration so a client can size its own
    /// bus/allocator state from the server instead of hardcoding it:
    /// `/server_query.reply [audio_buses, control_buses, output_channels,
    /// block_size, nominal_sr, actual_sr, input_channels, max_nodes,
    /// max_buffers, max_graph_children, max_ugen_inputs, taps, tap_frames,
    /// max_frame]`. The first six fields are stable; the boot-time capacities
    /// (S7), the tap region shape and the stream-transport frame ceiling
    /// (M25 — what a client should size bulk requests like `/buffer_getRange` chunks
    /// from) are appended so older clients that read only the six keep
    /// working.
    fn send_server_query(&mut self, to: ClientId) {
        let limits = self.handle.limits;
        let (taps, tap_frames) = self
            .handle
            .segment()
            .map_or((0, 0), |s| (s.taps(), s.tap_frames()));
        let args = vec![
            OscType::Int(self.handle.audio_buses as i32),
            OscType::Int(self.handle.control_buses().len() as i32),
            OscType::Int(self.handle.channels as i32),
            OscType::Int(crate::dsp::BLOCK_SIZE as i32),
            OscType::Double(self.info.nominal_sample_rate),
            OscType::Double(self.info.actual_sample_rate),
            OscType::Int(self.handle.input_channels as i32),
            OscType::Int(limits.max_nodes as i32),
            OscType::Int(limits.max_buffers as i32),
            OscType::Int(limits.max_group_children as i32),
            OscType::Int(limits.max_ugen_inputs as i32),
            OscType::Int(taps as i32),
            OscType::Int(tap_frames as i32),
            OscType::Int(self.max_frame.min(i32::MAX as usize) as i32),
        ];
        self.reply(to, "/server_query.reply", args);
    }

    /// M8: the sample-clock query. Replies `/clock_query.reply` with the engine's
    /// sample counter (int64 `h`), the actual sample rate (double `d`) and the
    /// server's OSC/NTP time captured with the counter (timetag `t`). The
    /// `(osc_time, sample)` pair is the master-clock **anchor**: a client maps
    /// its logical OSC time `T` to this server's sample axis with
    /// `S0 + (T − T0)·rate` and schedules with [`/sched_at`] (`Self::handle_sched_at`)
    /// directly in samples — see `docs/sample-clock.md`. Clients that only want
    /// the older two-field form ignore the trailing timetag. The counter counts
    /// *processed* samples: it runs a device buffer ahead of the speakers and
    /// pauses on xruns.
    fn handle_clock_query(&mut self, from: ClientId) {
        // Read the counter and the wall clock back-to-back so the published
        // anchor pairs the same instant (the sub-microsecond gap is negligible).
        let sample = self.handle.current_samples();
        let args = vec![
            OscType::Long(sample as i64),
            OscType::Double(self.info.actual_sample_rate),
            OscType::Time(self.now_ntp()),
        ];
        self.reply(from, "/clock_query.reply", args);
    }

    /// The `/transport_query.reply` payload: the grid plus the rolling state,
    /// `(origin_sample:int64, tempo:double, defined:int32, playing:int32,
    /// position:double)`. The first three fields are the original M22 grid reply
    /// (older clients read just those); `playing`/`position` are appended.
    fn transport_reply_args(&self) -> Vec<OscType> {
        let (origin, tempo, defined, playing, position) = match self.transport {
            Some(t) => (t.origin_sample, t.tempo, 1, t.playing as i32, t.position),
            None => (0, 0.0, 0, 0, 0.0),
        };
        vec![
            OscType::Long(origin),
            OscType::Double(tempo),
            OscType::Int(defined),
            OscType::Int(playing),
            OscType::Double(position),
        ]
    }

    /// Pushes the current transport state to every `/server_notify` client, so a
    /// responder on `/transport_query.reply` re-aligns or rolls its playhead live when
    /// the conductor changes the grid, plays, stops or locates — no polling.
    fn broadcast_transport(&self) {
        let push = self.transport_reply_args();
        for client in &self.clients {
            self.reply(*client, "/transport_query.reply", push.clone());
        }
    }

    /// `/transport_query` — reads the shared beat grid plus the rolling state.
    /// Replies `/transport_query.reply (origin_sample:int64, tempo:double,
    /// defined:int32, playing:int32, position:double)`, all zeros (and `defined`
    /// 0) when no grid is set.
    fn handle_transport_query(&mut self, from: ClientId) {
        let args = self.transport_reply_args();
        self.reply(from, "/transport_query.reply", args);
    }

    /// `/transport_set <origin_sample:int64> <tempo:double>` — sets the shared
    /// beat grid for phase-aligning several clients on the master sample clock
    /// (last writer wins), stopped at position 0, and replies `/done`. The grid
    /// is `beat b -> sample origin_sample + b·rate/tempo`; a client joins by
    /// reading it with [`Self::handle_transport_query`] and quantizing its start
    /// onto it. The server only stores/broadcasts it — in-memory (resets on
    /// restart), never scheduling audio from it.
    ///
    /// The rolling state (play/stop/locate) rides on top: see
    /// [`Self::handle_transport_play`]. Any change is **pushed** to every
    /// `/server_notify` client (the C13 responder path).
    fn handle_transport(&mut self, msg: &OscMessage, from: ClientId) {
        let origin = match msg.args.first() {
            Some(OscType::Long(v)) => *v,
            Some(OscType::Int(v)) => *v as i64,
            _ => {
                return self.fail(
                    from,
                    "/transport_set",
                    "expected (int64 originSample, double tempo)",
                );
            }
        };
        let tempo = match msg.args.get(1) {
            Some(OscType::Double(v)) => *v,
            Some(OscType::Float(v)) => *v as f64,
            _ => {
                return self.fail(
                    from,
                    "/transport_set",
                    "expected (int64 originSample, double tempo)",
                );
            }
        };
        if origin < 0 || tempo.is_nan() || tempo <= 0.0 {
            return self.fail(
                from,
                "/transport_set",
                "originSample must be >= 0 and tempo > 0",
            );
        }
        // Setting the grid (re)defines the transport: stopped, at position 0.
        self.transport = Some(Transport {
            origin_sample: origin,
            tempo,
            playing: false,
            position: 0.0,
        });
        self.reply(
            from,
            "/done",
            vec![OscType::String("/transport_set".into())],
        );
        self.broadcast_transport();
    }

    /// `/transport_play [position:double]` — start the transport rolling. With a
    /// `position` argument, playback starts from that song-position beat;
    /// without one, from where it last stopped/located. Every client's playhead
    /// obeys the broadcast (starting from `position`, quantized to the shared
    /// grid). Needs a grid defined first (`/transport_set`).
    fn handle_transport_play(&mut self, msg: &OscMessage, from: ClientId) {
        let Some(mut t) = self.transport else {
            return self.fail(from, "/transport_play", "no transport defined");
        };
        if let Some(pos) = msg.args.first() {
            match pos {
                OscType::Double(v) => t.position = *v,
                OscType::Float(v) => t.position = *v as f64,
                _ => return self.fail(from, "/transport_play", "expected (double position)"),
            }
        }
        t.playing = true;
        self.transport = Some(t);
        self.reply(
            from,
            "/done",
            vec![OscType::String("/transport_play".into())],
        );
        self.broadcast_transport();
    }

    /// `/transport_stop` — stop the transport. Every client's playhead halts at
    /// its current point; `position` holds for the next play.
    fn handle_transport_stop(&mut self, from: ClientId) {
        let Some(mut t) = self.transport else {
            return self.fail(from, "/transport_stop", "no transport defined");
        };
        t.playing = false;
        self.transport = Some(t);
        self.reply(
            from,
            "/done",
            vec![OscType::String("/transport_stop".into())],
        );
        self.broadcast_transport();
    }

    /// `/transport_locate <position:double>` — set the song position (where play
    /// starts or, while playing, seeks to). Every client's playhead locates to
    /// it; the `playing` flag is unchanged.
    fn handle_transport_locate(&mut self, msg: &OscMessage, from: ClientId) {
        let Some(mut t) = self.transport else {
            return self.fail(from, "/transport_locate", "no transport defined");
        };
        match msg.args.first() {
            Some(OscType::Double(v)) => t.position = *v,
            Some(OscType::Float(v)) => t.position = *v as f64,
            _ => return self.fail(from, "/transport_locate", "expected (double position)"),
        }
        self.transport = Some(t);
        self.reply(
            from,
            "/done",
            vec![OscType::String("/transport_locate".into())],
        );
        self.broadcast_transport();
    }

    /// M8: `/sched_at <int64 target> <blob packet>` — a timed bundle whose time
    /// is an absolute position on the **sample clock** instead of an NTP
    /// timetag (the OSC timetag format is NTP by spec, so sample targets get
    /// a container message rather than a reinterpreted tag; both front-ends
    /// feed the same engine queue and coexist freely). The blob is a complete
    /// OSC packet; all its leaf messages execute atomically at the target
    /// sample — nested bundle timetags inside the blob are **ignored**, one
    /// `/sched_at` is one instant. Past targets run at the start of the next
    /// block, like late NTP bundles.
    fn handle_sched_at(&mut self, msg: &OscMessage, from: ClientId) {
        let target = match msg.args.first() {
            Some(OscType::Long(t)) => *t,
            // Tolerated for hand-written clients; real targets outgrow i32
            // in under 13 hours at 48 kHz.
            Some(OscType::Int(t)) => *t as i64,
            _ => {
                return self.fail(
                    from,
                    "/sched_at",
                    "expected (int64 sampleTarget, blob packet)",
                );
            }
        };
        if target < 0 {
            return self.fail(from, "/sched_at", "sample target must be >= 0");
        }
        let Some(OscType::Blob(blob)) = msg.args.get(1) else {
            return self.fail(
                from,
                "/sched_at",
                "expected (int64 sampleTarget, blob packet)",
            );
        };
        let packet = match crate::osc::decode_packet(blob) {
            Ok(packet) => packet,
            Err(e) => return self.fail(from, "/sched_at", format!("bad packet blob: {e}")),
        };
        let mut cmds = Vec::new();
        self.sched_leaves(&packet, target, &mut cmds, from);
        if !cmds.is_empty()
            && self
                .handle
                .send(Cmd::Schedule {
                    time: target as u64,
                    cmds,
                })
                .is_err()
        {
            self.fail(from, "/sched_at", "command FIFO full");
        }
    }

    /// Translates every leaf message of a `/sched_at` blob, like
    /// [`Self::schedule_bundle`] does for NTP bundles: bad messages reply
    /// `/fail` individually, the rest still fire.
    fn sched_leaves(
        &mut self,
        packet: &OscPacket,
        target: i64,
        cmds: &mut Vec<Cmd>,
        from: ClientId,
    ) {
        match packet {
            OscPacket::Message(msg) => {
                tracing::trace!(target: crate::logging::OSC_TARGET, "{} {:?} (at sample {target})", msg.addr, msg.args);
                if let Err(e) = self.schedule_message(msg, cmds) {
                    self.fail(from, &msg.addr, e);
                }
            }
            OscPacket::Bundle(bundle) => {
                for inner in &bundle.content {
                    self.sched_leaves(inner, target, cmds, from);
                }
            }
        }
    }

    /// Gives `name` to `kind`, freeing it in the other two def kinds — in
    /// memory and on disk.
    ///
    /// A name identifies **one** def: sending a def under a name another kind
    /// holds replaces it, last one wins. Without this the two entries coexist
    /// and lookup order decides which answers, which is silently wrong
    /// everywhere the name is resolved — instancing, `/def_query`, and the bus
    /// usage the parallel scheduler reads.
    ///
    /// For a Faust def this runs at **submit** time, before the compile
    /// finishes, so a compile that then fails still leaves the name free. That
    /// is the honest reading of the request: the client said this name is a
    /// Faust def now.
    fn claim_def_name(&mut self, name: &str, kind: DefKind) {
        #[cfg(feature = "synth")]
        if kind != DefKind::Synth {
            self.translator.synth_defs.remove(name);
        }
        #[cfg(feature = "faust")]
        if kind != DefKind::Faust {
            self.translator.faust_defs.remove(name);
        }
        if kind != DefKind::Graph {
            self.translator.graph_defs.remove(name);
        }
        if let Some(store) = &self.store {
            store.remove_other_kinds(name, kind);
        }
    }

    /// `/def_send <family> <payload…>` — sends a def of any family: `"synth"`
    /// (one `SynthDefSpec` JSON blob), `"faust"` (a name and a def payload) or
    /// `"graph"` (one `GraphDefSpec` JSON blob). The family is a wire argument
    /// rather than three commands because it is already a datum of a def — it
    /// is what [`Self::handle_def_query`] reports back under the same name and
    /// the same three spellings.
    ///
    /// The ack echoes both: `/done "/def_send" <family>` (a faust compile,
    /// which finishes asynchronously, appends the def name).
    fn handle_def_send(&mut self, msg: &OscMessage, from: ClientId) {
        let Some(OscType::String(family)) = msg.args.first() else {
            return self.fail(
                from,
                "/def_send",
                "expected a family string (\"synth\", \"faust\" or \"graph\")",
            );
        };
        let family = family.clone();
        let rest = &msg.args[1..];
        match family.as_str() {
            "synth" => self.handle_def_send_synth(rest, from),
            "faust" => self.handle_def_send_faust(rest, from),
            "graph" => self.handle_def_send_graph(rest, from),
            other => self.fail(from, "/def_send", format!("unknown def family '{other}'")),
        }
    }

    fn handle_def_send_synth(&mut self, args: &[OscType], from: ClientId) {
        match self.translator.d_recv(args) {
            Ok(name) => {
                self.claim_def_name(&name, DefKind::Synth);
                if let Some(store) = &self.store
                    && !defstore::is_ephemeral(&name)
                    && let Some(spec) = synthdef_spec_bytes(args)
                    && let Err(e) = store.save_synthdef(&name, spec)
                {
                    error!("could not persist SynthDef '{name}': {e}");
                }
                self.reply(
                    from,
                    "/done",
                    vec![
                        OscType::String("/def_send".into()),
                        OscType::String("synth".into()),
                    ],
                );
            }
            Err(e) => self.fail(from, "/def_send", e),
        }
    }

    /// `/def_send graph <json>` (M18): load a GraphDef (validate + store), persist its
    /// spec verbatim, and reply `/done`. Cheap — no JIT, just validation.
    fn handle_def_send_graph(&mut self, args: &[OscType], from: ClientId) {
        match self.translator.d_graph(args) {
            Ok(name) => {
                self.claim_def_name(&name, DefKind::Graph);
                if let Some(store) = &self.store
                    && !defstore::is_ephemeral(&name)
                    && let Some(spec) = synthdef_spec_bytes(args)
                    && let Err(e) = store.save_graphdef(&name, spec)
                {
                    error!("could not persist GraphDef '{name}': {e}");
                }
                self.reply(
                    from,
                    "/done",
                    vec![
                        OscType::String("/def_send".into()),
                        OscType::String("graph".into()),
                    ],
                );
            }
            Err(e) => self.fail(from, "/def_send", e),
        }
    }

    fn handle_def_free(&mut self, msg: &OscMessage, from: ClientId) {
        if let Err(e) = self.translator.d_free(&msg.args) {
            return self.fail(from, "/def_free", e);
        }
        if let Some(store) = &self.store {
            for arg in &msg.args {
                if let OscType::String(name) = arg {
                    store.remove_synthdef(name);
                    store.remove_graphdef(name);
                    #[cfg(feature = "faust")]
                    crate::faust::cache::remove(store.faustdefs_dir(), name);
                }
            }
        }
    }

    /// M19: write the current MIDI bindings to disk after a mutation, if
    /// persistence is on. Best-effort; a write error is logged, never fatal.
    fn persist_bindings(&self) {
        if let Some(store) = &self.store
            && let Err(e) = store.save_bindings(&self.translator.midi.persist())
        {
            error!("could not persist MIDI bindings: {e}");
        }
    }

    /// Immediate form of every translator-covered command: translate (which
    /// also updates the M12 tree mirror and may append re-sort moves), then
    /// ship the whole batch.
    fn handle_via_translate(&mut self, msg: &OscMessage, from: ClientId) {
        let mut cmds = Vec::new();
        if let Err(e) = self.translator.translate(msg, &mut cmds) {
            return self.fail(from, &msg.addr, e);
        }
        for cmd in cmds {
            if self.handle.send(cmd).is_err() {
                return self.fail(from, &msg.addr, "command FIFO full");
            }
        }
    }

    /// The node tree as seen by the network-side mirror, in scsynth's
    /// `/group_queryTree.reply` format. Args: [groupID = 0, detail = 0]; detail 1
    /// includes control names and values (scsynth's flag), detail 2 also the
    /// maps and inferred bus lists, which makes each entry a full node info.
    fn handle_group_query_tree(&mut self, msg: &OscMessage, from: ClientId) {
        let group = int_arg(&msg.args, 0).unwrap_or(0);
        let detail = int_arg(&msg.args, 1).unwrap_or(0).clamp(0, 2);
        match self.translator.query_tree(group, detail) {
            Ok(args) => self.reply(from, "/group_queryTree.reply", args),
            Err(e) => self.fail(from, "/group_queryTree", e),
        }
    }

    /// Per-node detail: replies `/node_query.reply` for each queried node ID (scsynth's
    /// `/node_query`, extended with the def name, controls, maps and inferred
    /// bus usage — see [`CmdTranslator::node_info`]). An id the server does
    /// not hold answers with an absent record, not `/fail`: only a malformed
    /// request is a protocol error.
    fn handle_node_query(&mut self, msg: &OscMessage, from: ClientId) {
        for arg in &msg.args {
            let OscType::Int(id) = arg else {
                return self.fail(from, "/node_query", "expected int node ids");
            };
            let args = self.translator.node_info(*id);
            self.reply(from, "/node_query.reply", args);
        }
    }

    /// `/group_query path...`: resolves each path to the node it names,
    /// replying `/group_query.reply <path> <nodeID>` — the one place a path is
    /// interpreted. A path nothing answers to resolves to `-1` (absence is a
    /// state, as in `/node_query`), so one dead path does not abort the rest.
    fn handle_group_query(&mut self, msg: &OscMessage, from: ClientId) {
        for arg in &msg.args {
            let OscType::String(path) = arg else {
                return self.fail(from, "/group_query", "expected string paths");
            };
            let id = self.translator.resolve_path(path);
            self.reply(
                from,
                "/group_query.reply",
                vec![OscType::String(path.clone()), OscType::Int(id)],
            );
        }
    }

    /// M12 debug: the inferred bus graph of one group as a string reply.
    fn handle_group_dump_graph(&mut self, msg: &OscMessage, from: ClientId) {
        let group = int_arg(&msg.args, 0).unwrap_or(0);
        match self.translator.dump_graph(group) {
            Ok(dump) => self.reply(
                from,
                "/group_dumpGraph.reply",
                vec![OscType::Int(group), OscType::String(dump)],
            ),
            Err(e) => self.fail(from, "/group_dumpGraph", e),
        }
    }

    /// `/bus_stream periodMs busIndex...`: subscribes this client to a periodic
    /// `/bus_set` snapshot of the listed control buses — the network counterpart
    /// of reading the shared-memory segment, for clients that cannot map it (a
    /// browser GUI host's meters/scopes over WebSocket). One subscription per
    /// client, replaced on every call; `periodMs <= 0` or an empty list
    /// cancels. Acks `/done "/bus_stream"`, then sends the first snapshot
    /// immediately and the rest from the run loop. Not schedulable in timed
    /// bundles. Subscriptions die with their TCP/WS connection; UDP and ring
    /// clients cancel explicitly (same posture as `/server_notify`).
    fn handle_bus_stream(&mut self, msg: &OscMessage, from: ClientId) {
        let Some(OscType::Int(period_ms)) = msg.args.first() else {
            return self.fail(from, "/bus_stream", "expected int periodMs");
        };
        let mut buses = Vec::with_capacity(msg.args.len().saturating_sub(1));
        for arg in &msg.args[1..] {
            let OscType::Int(index) = arg else {
                return self.fail(from, "/bus_stream", "expected int bus indices");
            };
            if *index < 0 {
                return self.fail(from, "/bus_stream", "bus index must be non-negative");
            }
            buses.push(*index);
        }
        if buses.len() > MAX_STREAM_BUSES {
            return self.fail(
                from,
                "/bus_stream",
                format!("at most {MAX_STREAM_BUSES} bus indices per subscription"),
            );
        }
        self.streams.retain(|s| s.client != from);
        self.reply(from, "/done", vec![OscType::String("/bus_stream".into())]);
        if *period_ms > 0 && !buses.is_empty() {
            let period = Duration::from_millis(*period_ms as u64).max(MIN_STREAM_PERIOD);
            self.streams.push(BusStream {
                client: from,
                period,
                buses,
                next_due: self.mono_secs() + period.as_secs_f64(),
            });
            // The immediate snapshot: the client paints without waiting a period.
            let args = self.stream_args(self.streams.len() - 1);
            self.reply(from, "/bus_stream.reply", args);
        }
        self.retune_timeout();
    }

    /// Sends every due stream its `/bus_stream.reply` snapshot. Called once per run-loop
    /// iteration; the socket timeout is tuned so an idle loop still ticks at
    /// the fastest subscribed period (see [`Self::retune_timeout`]). Reading a
    /// control bus is one relaxed atomic load — no engine round-trip.
    fn pump_streams(&mut self) {
        if self.streams.is_empty() {
            return;
        }
        let now = self.mono_secs();
        for i in 0..self.streams.len() {
            if now < self.streams[i].next_due {
                continue;
            }
            let client = self.streams[i].client;
            let args = self.stream_args(i);
            self.reply(client, "/bus_stream.reply", args);
            // Rebase on `now` (no catch-up bursts after a stall).
            let period = self.streams[i].period;
            self.streams[i].next_due = now + period.as_secs_f64();
        }
    }

    /// The `(busIndex, value)` pairs of stream `i`'s snapshot.
    fn stream_args(&self, i: usize) -> Vec<OscType> {
        let buses = &self.streams[i].buses;
        let mut args = Vec::with_capacity(buses.len() * 2);
        for &bus in buses {
            args.push(OscType::Int(bus));
            args.push(OscType::Float(
                self.handle.control_buses().get(bus as usize),
            ));
        }
        args
    }

    /// Starts recording audio bus `bus`, if it is not already: picks a free
    /// ring, tells the engine to append that bus to it every block, and counts
    /// one more watcher. Idempotent per watcher — two views of the same bus
    /// share one ring — and the caller never learns the index: the segment's
    /// directory maps the bus to it (see [`Segment::tap_of_bus`]).
    ///
    /// [`Segment::tap_of_bus`]: crate::server::ipc::Segment::tap_of_bus
    fn watch_bus(&mut self, bus: i32) -> Result<(), String> {
        let Some(segment) = self.handle.segment() else {
            return Err("no tap region (server started with --taps 0)".into());
        };
        let taps = segment.taps();
        let audio_buses = self.handle.audio_buses;
        if bus < 0 || bus as usize >= audio_buses {
            return Err(format!("bus must be in range 0..{audio_buses}"));
        }
        self.tap_rings.resize(taps, -1);
        self.tap_refs.resize(taps, 0);
        if let Some(ring) = self.tap_rings.iter().position(|&b| b == bus) {
            self.tap_refs[ring] += 1;
            return Ok(());
        }
        let Some(ring) = self.tap_rings.iter().position(|&b| b < 0) else {
            return Err(format!("all {taps} audio taps are in use"));
        };
        if self.handle.send(Cmd::SetTap { tap: ring, bus }).is_err() {
            return Err("command FIFO full".into());
        }
        self.tap_rings[ring] = bus;
        self.tap_refs[ring] = 1;
        Ok(())
    }

    /// Drops one watcher of `bus`, freeing its ring when the last one goes.
    fn release_bus(&mut self, bus: i32) {
        let Some(ring) = self.tap_rings.iter().position(|&b| b == bus) else {
            return;
        };
        self.tap_refs[ring] = self.tap_refs[ring].saturating_sub(1);
        if self.tap_refs[ring] == 0 {
            // Best effort: a full FIFO leaves the ring recording, and the next
            // watcher of this bus reuses it rather than taking a second one.
            if self.handle.send(Cmd::SetTap { tap: ring, bus: -1 }).is_ok() {
                self.tap_rings[ring] = -1;
            } else {
                self.tap_refs[ring] = 1;
            }
        }
    }

    /// `/bus_tap bus watch`: asks the server to make audio bus `bus` readable —
    /// `watch = 1` starts, `0` stops. **The bus is the only number a client
    /// names**: which of the segment's rings carries it is the server's own
    /// bookkeeping, published in the segment's bus directory for whoever reads
    /// the samples (a GUI host's oscilloscope, with zero messages per frame).
    /// Watches count, so two views of one bus share a ring and the last one to
    /// stop frees it. No ack (the same posture as `/node_map`: it only flips
    /// routing state); sequence with `/server_sync` when needed. Fails without a tap
    /// region (server started with `--taps 0`) or when every ring is taken.
    fn handle_bus_tap(&mut self, msg: &OscMessage, from: ClientId) {
        let (Some(OscType::Int(bus)), Some(OscType::Int(watch))) =
            (msg.args.first(), msg.args.get(1))
        else {
            return self.fail(from, "/bus_tap", "expected int bus, int watch");
        };
        // Both directions answer the same way about an impossible request, so
        // a client learns it cannot watch anything from the first call.
        if self.handle.segment().is_none() {
            return self.fail(
                from,
                "/bus_tap",
                "no tap region (server started with --taps 0)",
            );
        }
        let audio_buses = self.handle.audio_buses;
        if *bus < 0 || *bus as usize >= audio_buses {
            return self.fail(
                from,
                "/bus_tap",
                format!("bus must be in range 0..{audio_buses}"),
            );
        }
        if *watch == 0 {
            return self.release_bus(*bus);
        }
        if let Err(why) = self.watch_bus(*bus) {
            self.fail(from, "/bus_tap", why);
        }
    }

    /// `/bus_tapStream periodMs frames bus...`: subscribes this client to a
    /// periodic `/bus_tapStream.reply` snapshot — the newest `frames` samples of each
    /// listed **audio bus** — the network counterpart of reading the segment's
    /// tap rings, for clients (a browser oscilloscope) that cannot map it. The
    /// subscription *is* the watch: it starts recording each bus it lists and
    /// stops when it is replaced, cancelled or its connection dies, so a
    /// streaming client never issues `/bus_tap` at all. One subscription per
    /// client, replaced on every call; `periodMs <= 0` or an empty bus list
    /// cancels. Acks `/done "/bus_tapStream"`, then sends the first snapshot
    /// immediately and the rest from the run loop. Not schedulable in timed
    /// bundles. Subscriptions die with their TCP/WS connection, like
    /// `/bus_stream`.
    fn handle_bus_tap_stream(&mut self, msg: &OscMessage, from: ClientId) {
        let (Some(OscType::Int(period_ms)), Some(OscType::Int(frames))) =
            (msg.args.first(), msg.args.get(1))
        else {
            return self.fail(from, "/bus_tapStream", "expected int periodMs, int frames");
        };
        let Some(segment) = self.handle.segment() else {
            return self.fail(
                from,
                "/bus_tapStream",
                "no tap region (server started with --taps 0)",
            );
        };
        let audio_buses = self.handle.audio_buses;
        let mut buses = Vec::with_capacity(msg.args.len().saturating_sub(2));
        for arg in &msg.args[2..] {
            let OscType::Int(bus) = arg else {
                return self.fail(from, "/bus_tapStream", "expected int bus indices");
            };
            if *bus < 0 || *bus as usize >= audio_buses {
                return self.fail(
                    from,
                    "/bus_tapStream",
                    format!("bus out of range 0..{audio_buses}"),
                );
            }
            buses.push(*bus);
        }
        if buses.len() > MAX_STREAM_TAPS {
            return self.fail(
                from,
                "/bus_tapStream",
                format!("at most {MAX_STREAM_TAPS} buses per subscription"),
            );
        }
        // Clamp, don't fail, the window: to the client's transport bound and
        // to half the tap ring (the tear-free bound of `tap_read_latest`). A
        // stream client may fill a whole frame (minus the OSC envelope); a
        // datagram-bounded one keeps the 32 KB blob cap.
        let transport_cap = match from {
            ClientId::Tcp(_) | ClientId::Ws(_) => self.max_frame.saturating_sub(256) / 4,
            ClientId::Udp(_) | ClientId::Ring => MAX_TAP_WINDOW,
        };
        let frames = (*frames).max(1) as usize;
        let frames = frames.min(transport_cap).min(segment.tap_frames() / 2);
        // The new subscription's watches are taken before the old one's are
        // dropped, so re-subscribing to the same bus never stops recording it.
        let wanted = if *period_ms > 0 {
            buses.clone()
        } else {
            Vec::new()
        };
        for bus in &wanted {
            if let Err(why) = self.watch_bus(*bus) {
                for taken in wanted.iter().take_while(|b| *b != bus) {
                    self.release_bus(*taken);
                }
                return self.fail(from, "/bus_tapStream", why);
            }
        }
        self.drop_tap_streams(|s| s.client == from);
        self.reply(
            from,
            "/done",
            vec![OscType::String("/bus_tapStream".into())],
        );
        if !wanted.is_empty() {
            let period = Duration::from_millis(*period_ms as u64).max(MIN_STREAM_PERIOD);
            self.tap_streams.push(TapStream {
                client: from,
                period,
                frames,
                buses,
                next_due: self.mono_secs() + period.as_secs_f64(),
            });
            // The immediate snapshot: the client paints without waiting a
            // period (taps that have not yet filled a window send nothing).
            self.send_tap_snapshots(self.tap_streams.len() - 1);
        }
        self.retune_timeout();
    }

    /// Removes the tap subscriptions matching `doomed` and releases the watch
    /// each held on its buses — the one place a subscription's recording stops,
    /// whether it was replaced, cancelled or lost with its connection.
    fn drop_tap_streams(&mut self, doomed: impl Fn(&TapStream) -> bool) {
        let mut released = Vec::new();
        self.tap_streams.retain(|s| {
            if doomed(s) {
                released.extend_from_slice(&s.buses);
                false
            } else {
                true
            }
        });
        for bus in released {
            self.release_bus(bus);
        }
    }

    /// Sends every due tap stream its `/bus_tapStream.reply` snapshots. Called once per
    /// run-loop iteration, like [`Self::pump_streams`]. Reading a tap ring is
    /// a lock-free shared-memory copy — no engine round-trip.
    fn pump_tap_streams(&mut self) {
        if self.tap_streams.is_empty() {
            return;
        }
        let now = self.mono_secs();
        for i in 0..self.tap_streams.len() {
            if now < self.tap_streams[i].next_due {
                continue;
            }
            self.send_tap_snapshots(i);
            // Rebase on `now` (no catch-up bursts after a stall).
            let period = self.tap_streams[i].period;
            self.tap_streams[i].next_due = now + period.as_secs_f64();
        }
    }

    /// One `/bus_tapStream.reply tap endPosition blob` per tap of stream `i` that has a
    /// full window: `endPosition` is the tap's stream position (total samples
    /// written) at the window's end — consecutive snapshots overlap or gap by
    /// exactly the position delta — and the blob is the window's raw
    /// little-endian `f32` samples.
    fn send_tap_snapshots(&mut self, i: usize) {
        let Some(segment) = self.handle.segment().cloned() else {
            return;
        };
        let client = self.tap_streams[i].client;
        let frames = self.tap_streams[i].frames;
        self.tap_buf.resize(frames, 0.0);
        for k in 0..self.tap_streams[i].buses.len() {
            let bus = self.tap_streams[i].buses[k];
            // The bus is the key here too: the ring it landed in is looked up,
            // never carried in the subscription.
            let Some(tap) = segment.tap_of_bus(bus as usize) else {
                continue;
            };
            let Some(end) = segment.tap_read_latest(tap, &mut self.tap_buf) else {
                continue;
            };
            let mut bytes = Vec::with_capacity(frames * 4);
            for s in &self.tap_buf {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
            self.reply(
                client,
                "/bus_tapStream.reply",
                vec![
                    OscType::Int(bus),
                    OscType::Long(end as i64),
                    OscType::Blob(bytes),
                ],
            );
        }
    }

    /// Retunes the socket read timeout — the run loop's idle tick — to the
    /// fastest subscribed stream period, so streams keep their cadence without
    /// traffic. The 2 ms IPC poll (`attach_ipc`) is faster than any allowed
    /// period and wins unconditionally; without streams the tick falls back to
    /// the GC interval.
    fn retune_timeout(&self) {
        if self.ipc.is_some() {
            return;
        }
        let timeout = self
            .streams
            .iter()
            .map(|s| s.period)
            .chain(self.tap_streams.iter().map(|s| s.period))
            .min()
            .map_or(GC_INTERVAL, |p| p.min(GC_INTERVAL));
        let Some(socket) = &self.socket else {
            // Headless: the host's own step cadence is the tick.
            return;
        };
        if let Err(e) = socket.set_read_timeout(Some(timeout)) {
            warn!("failed to retune the socket timeout: {e}");
        }
    }

    /// Forgets per-client state (bus streams, `/server_notify` registrations) for
    /// TCP/WS connections that closed since the last pass. UDP and ring
    /// clients have no disconnect signal; their state goes on explicit
    /// cancel/`/server_notify 0` or `/server_quit`, as in scsynth.
    fn prune_disconnected(&mut self) {
        let mut gone: Vec<ClientId> = Vec::new();
        if let Some(hub) = &mut self.tcp {
            gone.extend(hub.take_disconnects().into_iter().map(ClientId::Tcp));
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(hub) = &mut self.ws {
            gone.extend(hub.take_disconnects().into_iter().map(ClientId::Ws));
        }
        if gone.is_empty() {
            return;
        }
        self.streams.retain(|s| !gone.contains(&s.client));
        self.drop_tap_streams(|s| gone.contains(&s.client));
        self.clients.retain(|c| !gone.contains(c));
        self.retune_timeout();
    }

    /// Control buses are shared atomics: set directly, no engine round-trip.
    fn handle_bus_set(&mut self, msg: &OscMessage, from: ClientId) {
        if msg.args.is_empty() || !msg.args.len().is_multiple_of(2) {
            return self.fail(from, "/bus_set", "expected (busIndex, value) pairs");
        }
        for pair in msg.args.chunks(2) {
            let (OscType::Int(index), Some(value)) = (&pair[0], float_value(&pair[1])) else {
                return self.fail(from, "/bus_set", "expected int bus index and number value");
            };
            if *index < 0 {
                return self.fail(from, "/bus_set", "bus index must be non-negative");
            }
            self.handle.control_buses().set(*index as usize, value);
        }
    }

    /// Replies with a `/bus_get.reply` message carrying (busIndex, value) pairs.
    fn handle_bus_get(&mut self, msg: &OscMessage, from: ClientId) {
        let mut args = Vec::with_capacity(msg.args.len() * 2);
        for arg in &msg.args {
            let OscType::Int(index) = arg else {
                return self.fail(from, "/bus_get", "expected int bus indices");
            };
            if *index < 0 {
                return self.fail(from, "/bus_get", "bus index must be non-negative");
            }
            args.push(OscType::Int(*index));
            args.push(OscType::Float(
                self.handle.control_buses().get(*index as usize),
            ));
        }
        self.reply(from, "/bus_get.reply", args);
    }

    /// `/bus_setRange busIndex numBuses val...`: sets a consecutive range of control
    /// buses (one or more groups). Immediate form writes the shared atomics.
    fn handle_bus_set_range(&mut self, msg: &OscMessage, from: ClientId) {
        let mut rest = msg.args.as_slice();
        while !rest.is_empty() {
            let [OscType::Int(base), OscType::Int(count), tail @ ..] = rest else {
                return self.fail(
                    from,
                    "/bus_setRange",
                    "expected (busIndex, numBuses, values...) groups",
                );
            };
            let (Ok(base), Ok(count)) = (usize::try_from(*base), usize::try_from(*count)) else {
                return self.fail(from, "/bus_setRange", "bus index and numBuses must be >= 0");
            };
            if tail.len() < count {
                return self.fail(from, "/bus_setRange", "fewer values than numBuses");
            }
            for (offset, value) in tail[..count].iter().enumerate() {
                let Some(value) = float_value(value) else {
                    return self.fail(from, "/bus_setRange", "expected number values");
                };
                self.handle.control_buses().set(base + offset, value);
            }
            rest = &tail[count..];
        }
    }

    /// `/bus_getRange busIndex numBuses ...`: replies `/bus_getRange.reply` with each requested
    /// range expanded to `(busIndex, numBuses, val0, val1, ...)`.
    fn handle_bus_get_range(&mut self, msg: &OscMessage, from: ClientId) {
        if msg.args.is_empty() || !msg.args.len().is_multiple_of(2) {
            return self.fail(from, "/bus_getRange", "expected (busIndex, numBuses) pairs");
        }
        let mut args = Vec::new();
        for pair in msg.args.chunks(2) {
            let [OscType::Int(base), OscType::Int(count)] = pair else {
                return self.fail(from, "/bus_getRange", "expected int busIndex and numBuses");
            };
            let (Ok(base), Ok(count)) = (usize::try_from(*base), usize::try_from(*count)) else {
                return self.fail(from, "/bus_getRange", "bus index and numBuses must be >= 0");
            };
            args.push(OscType::Int(base as i32));
            args.push(OscType::Int(count as i32));
            for offset in 0..count {
                args.push(OscType::Float(
                    self.handle.control_buses().get(base + offset),
                ));
            }
        }
        self.reply(from, "/bus_getRange.reply", args);
    }

    /// `/bus_fill busIndex numBuses value ...`: fills a consecutive range of
    /// control buses with one value (groups of three).
    fn handle_bus_fill(&mut self, msg: &OscMessage, from: ClientId) {
        if msg.args.is_empty() || !msg.args.len().is_multiple_of(3) {
            return self.fail(
                from,
                "/bus_fill",
                "expected (busIndex, numBuses, value) triples",
            );
        }
        for group in msg.args.chunks(3) {
            let [OscType::Int(base), OscType::Int(count), val] = group else {
                return self.fail(from, "/bus_fill", "expected int busIndex and numBuses");
            };
            let (Ok(base), Ok(count)) = (usize::try_from(*base), usize::try_from(*count)) else {
                return self.fail(from, "/bus_fill", "bus index and numBuses must be >= 0");
            };
            let Some(value) = float_value(val) else {
                return self.fail(from, "/bus_fill", "expected number value");
            };
            for offset in 0..count {
                self.handle.control_buses().set(base + offset, value);
            }
        }
    }

    /// `/synth_get nodeID control...` / `/synth_getRange nodeID control numControls...`:
    /// reads a synth's current control values from the mirror and replies
    /// `/node_set nodeID control value ...` (`/synth_getRange` echoes each range's
    /// `(control, numControls, val...)`), the query counterpart of `/node_set`.
    fn handle_synth_get(&mut self, msg: &OscMessage, from: ClientId, ranged: bool) {
        let addr = if ranged {
            "/synth_getRange"
        } else {
            "/synth_get"
        };
        let Some(OscType::Int(id)) = msg.args.first() else {
            return self.fail(from, addr, "expected: nodeID, then controls");
        };
        let Some(def) = self.translator.node_defs.get(id).cloned() else {
            return self.fail(from, addr, format!("synth {id} not found"));
        };
        let Some((_, controls)) = self.translator.mirror.synth_info(*id) else {
            return self.fail(from, addr, format!("node {id} is not a synth"));
        };
        let mut args = vec![OscType::Int(*id)];
        let read = |index: u32| -> Result<f32, String> {
            controls
                .get(index as usize)
                .copied()
                .ok_or_else(|| format!("control index {index} out of range"))
        };
        if ranged {
            for pair in msg.args[1..].chunks(2) {
                let (Some(base), Some(OscType::Int(count))) =
                    (pair.first().and_then(|a| control_key(a, &def)), pair.get(1))
                else {
                    return self.fail(from, addr, "expected (control, numControls) pairs");
                };
                let Ok(count) = u32::try_from(*count) else {
                    return self.fail(from, addr, "numControls must be >= 0");
                };
                args.push(OscType::Int(base as i32));
                args.push(OscType::Int(count as i32));
                for offset in 0..count {
                    match read(base + offset) {
                        Ok(v) => args.push(OscType::Float(v)),
                        Err(e) => return self.fail(from, addr, e),
                    }
                }
            }
        } else {
            for arg in &msg.args[1..] {
                let Some(index) = control_key(arg, &def) else {
                    return self.fail(from, addr, "unknown control");
                };
                match read(index) {
                    Ok(v) => {
                        args.push(OscType::Int(index as i32));
                        args.push(OscType::Float(v));
                    }
                    Err(e) => return self.fail(from, addr, e),
                }
            }
        }
        self.reply(from, "/node_set", args);
    }

    /// `/synth_forgetId nodeID...`: in scsynth this releases the integer node IDs so the
    /// server may reuse them. Clausters allocates IDs per client (auto IDs are
    /// server-assigned, negative-free), never reclaims an in-use ID, and never
    /// reuses a freed one under a live node, so there is nothing to release; we
    /// validate the IDs name live synths and acknowledge. Deliberate deviation
    /// (the plan's "compatibility of model, not literal copy").
    fn handle_synth_forget_id(&mut self, msg: &OscMessage, from: ClientId) {
        if msg.args.is_empty() {
            return self.fail(from, "/synth_forgetId", "expected node IDs");
        }
        for arg in &msg.args {
            let OscType::Int(id) = arg else {
                return self.fail(from, "/synth_forgetId", "expected int node IDs");
            };
            if !self.translator.node_defs.contains_key(id) {
                return self.fail(from, "/synth_forgetId", format!("synth {id} not found"));
            }
        }
        self.reply(
            from,
            "/done",
            vec![OscType::String("/synth_forgetId".into())],
        );
    }

    /// `/node_trace nodeID...`: debug-traces a node by logging its current control
    /// values (from the mirror) to the server console — the introspection
    /// counterpart of scsynth's per-block node trace. Network-thread only, no
    /// reply (matches scsynth).
    fn handle_node_trace(&mut self, msg: &OscMessage, from: ClientId) {
        for arg in &msg.args {
            let OscType::Int(id) = arg else {
                return self.fail(from, "/node_trace", "expected int node IDs");
            };
            match self.translator.mirror.synth_info(*id) {
                Some((name, controls)) => {
                    info!(target: crate::logging::OSC_TARGET, "/node_trace node {id} synth {name:?} controls {controls:?}");
                }
                None if self.translator.mirror.get(*id).is_some() => {
                    let children = self.translator.mirror.children(*id).unwrap_or(&[]);
                    info!(target: crate::logging::OSC_TARGET, "/node_trace node {id} group children {children:?}");
                }
                None => {
                    info!(target: crate::logging::OSC_TARGET, "/node_trace node {id} not found")
                }
            }
        }
    }

    /// `/buffer_close bufnum`: closes a soundfile a streaming buffer left open
    /// (scsynth pairs this with `DiskIn`/`DiskOut`). Clausters has no streaming
    /// buffers yet — every `/buffer_read`/`/buffer_write` reads or writes the whole file
    /// and closes it — so there is never an open handle: this validates the
    /// buffer is live and acknowledges, forward-compatible with the future
    /// streaming UGens.
    fn handle_buffer_close(&mut self, msg: &OscMessage, from: ClientId) {
        let Some(OscType::Int(index)) = msg.args.first() else {
            return self.fail(from, "/buffer_close", "expected int buffer index");
        };
        let live = usize::try_from(*index)
            .ok()
            .and_then(|i| self.translator.buffers.get(i))
            .is_some_and(Option::is_some);
        if live {
            self.reply(
                from,
                "/done",
                vec![
                    OscType::String("/buffer_close".into()),
                    OscType::Int(*index),
                ],
            );
        } else {
            self.fail(
                from,
                "/buffer_close",
                format!("buffer {index} not allocated"),
            );
        }
    }

    /// `/def_load path`: loads a SynthDef from a JSON spec file on disk (the
    /// Clausters def format — the same body `/def_send synth` carries), on demand,
    /// complementing the boot-time reload. GraphDefs load through `/def_send graph`.
    fn handle_def_load(&mut self, msg: &OscMessage, from: ClientId) {
        let Some(OscType::String(path)) = msg.args.first() else {
            return self.fail(from, "/def_load", "expected string path");
        };
        match self.load_synthdef_file(std::path::Path::new(path)) {
            Ok(()) => self.reply(from, "/done", vec![OscType::String("/def_load".into())]),
            Err(e) => self.fail(from, "/def_load", e),
        }
    }

    /// `/def_loadDir dir`: loads every `*.json` SynthDef spec in a directory. A
    /// single unreadable/invalid file fails the whole command (like scsynth
    /// aborting on a bad def), naming the offending file.
    fn handle_def_load_dir(&mut self, msg: &OscMessage, from: ClientId) {
        let Some(OscType::String(dir)) = msg.args.first() else {
            return self.fail(from, "/def_loadDir", "expected string directory");
        };
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) => return self.fail(from, "/def_loadDir", format!("{dir}: {e}")),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json")
                && let Err(e) = self.load_synthdef_file(&path)
            {
                return self.fail(from, "/def_loadDir", e);
            }
        }
        self.reply(from, "/done", vec![OscType::String("/def_loadDir".into())]);
    }

    /// Reads one SynthDef spec file, compiles it through the `/def_send synth` path and
    /// persists it under its name. Shared by `/def_load` and `/def_loadDir`.
    fn load_synthdef_file(&mut self, path: &std::path::Path) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let args = [OscType::Blob(bytes.clone())];
        let name = self
            .translator
            .d_recv(&args)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if let Some(store) = &self.store
            && let Err(e) = store.save_synthdef(&name, &bytes)
        {
            error!("could not persist SynthDef '{name}': {e}");
        }
        Ok(())
    }

    /// `/sched_clear`: flushes every pending timed bundle from the engine's
    /// schedule queue. The bundles' heap (boxed synths and the `Vec` shells)
    /// leaves through the garbage FIFO, so nothing is dropped on the audio
    /// thread. Replies `/done`.
    fn handle_sched_clear(&mut self, from: ClientId) {
        if self.handle.send(Cmd::ClearSched).is_err() {
            return self.fail(from, "/sched_clear", "command FIFO full");
        }
        self.reply(from, "/done", vec![OscType::String("/sched_clear".into())]);
    }

    /// `/server_errorMode mode`: sets the error-posting mode. `1` posts command errors to
    /// the server console (the default), `0` silences them. The `/fail` OSC
    /// reply is always sent regardless — clients rely on it; only the
    /// server-side console logging is gated. scsynth's bundle-local `-1`/`-2`
    /// are not separately supported (deliberate deviation): the persistent
    /// `0`/`1` toggle is the model that fits our logging.
    fn handle_server_error_mode(&mut self, msg: &OscMessage, from: ClientId) {
        match msg.args.first() {
            Some(OscType::Int(mode)) => {
                self.post_errors = *mode != 0;
            }
            _ => self.fail(
                from,
                "/server_errorMode",
                "expected int mode (0 = off, 1 = on)",
            ),
        }
    }

    /// `/server_cmd name args...`: a server-wide, typed command — the discoverable
    /// replacement for scsynth's untyped `/server_cmd`. `name` selects a handler from
    /// the built-in registry; unknown names `/fail` with the offending name.
    /// The mechanism exists for future server commands; the built-in `ping`
    /// (replies `/done /server_cmd ping`) proves the surface.
    fn handle_server_cmd(&mut self, msg: &OscMessage, from: ClientId) {
        let Some(OscType::String(name)) = msg.args.first() else {
            return self.fail(from, "/server_cmd", "expected string command name");
        };
        match name.as_str() {
            "ping" => self.reply(
                from,
                "/done",
                vec![
                    OscType::String("/server_cmd".into()),
                    OscType::String("ping".into()),
                ],
            ),
            other => self.fail(
                from,
                "/server_cmd",
                format!("unknown server command {other:?}"),
            ),
        }
    }

    /// Drains finished NRT jobs: installs/clears buffers in the engine and
    /// the mirror, and sends the async `/done cmd bufnum` / `/fail` replies.
    /// Installs a host-built buffer at `index`: the network-side mirror and
    /// the engine swap, exactly the `NrtAction::Install` path minus the OSC
    /// reply. The embed `b_load` door (B1): a headless host hands the server
    /// samples it decoded itself (the browser's `/buffer_allocRead` replacement,
    /// where there is no filesystem).
    pub fn install_buffer(
        &mut self,
        index: usize,
        buffer: Arc<crate::dsp::buffer::Buffer>,
    ) -> Result<(), String> {
        if index >= self.translator.buffers.len() {
            return Err(format!(
                "buffer index {index} out of range (max {})",
                self.translator.buffers.len() - 1
            ));
        }
        self.translator.buffers[index] = Some(Arc::clone(&buffer));
        self.handle
            .send(Cmd::SetBuffer {
                index,
                buffer: Some(buffer),
            })
            .map_err(|_| "command FIFO full".to_string())
    }

    fn collect_nrt_results(&mut self) {
        while let Some(result) = self.nrt.try_result() {
            self.nrt_drained += 1;
            let action = match result.outcome {
                Ok(action) => action,
                Err(error) => {
                    self.fail(result.client, result.cmd, error);
                    continue;
                }
            };
            let index = result.index as usize;
            let swap = match action {
                NrtAction::Install(buffer) => {
                    self.translator.buffers[index] = Some(Arc::clone(&buffer));
                    Some(Some(buffer))
                }
                NrtAction::Clear => {
                    self.translator.buffers[index] = None;
                    Some(None)
                }
                NrtAction::None => None,
            };
            if let Some(buffer) = swap
                && self.handle.send(Cmd::SetBuffer { index, buffer }).is_err()
            {
                self.fail(result.client, result.cmd, "command FIFO full");
                continue;
            }
            self.reply(
                result.client,
                "/done",
                vec![
                    OscType::String(result.cmd.into()),
                    OscType::Int(result.index),
                ],
            );
        }
        self.resolve_syncs();
    }

    /// Any of the async `/buffer_*` commands: parsing is shared with the NRT
    /// renderer; the job runs on the NRT thread. `/buffer_free` also travels
    /// through the queue so it cannot overtake a pending alloc/read on the
    /// same index.
    fn handle_buffer_cmd(&mut self, msg: &OscMessage, from: ClientId, cmd: &'static str) {
        let (index, job) = match parse_buffer_msg(
            cmd,
            &msg.args,
            &self.translator.buffers,
            self.info.nominal_sample_rate,
        ) {
            Ok(parsed) => parsed,
            Err(e) => return self.fail(from, cmd, e),
        };
        self.submit_nrt(cmd, index, from, job);
    }

    /// `/buffer_gen bufnum cmd ...`: fills a buffer through the wavetable/generator
    /// path (see [`parse_buffer_gen`]). Async on the NRT queue, in submission order
    /// with the other `/buffer_*` commands, replying `/done`/`/fail` like them.
    fn handle_buffer_gen(&mut self, msg: &OscMessage, from: ClientId) {
        let (index, job) = match parse_buffer_gen(&msg.args, &self.translator.buffers) {
            Ok(parsed) => parsed,
            Err(e) => return self.fail(from, "/buffer_gen", e),
        };
        self.submit_nrt("/buffer_gen", index, from, job);
    }

    /// Queues a built NRT job, failing back to the client if the thread is gone.
    fn submit_nrt(&mut self, cmd: &'static str, index: i32, from: ClientId, job: NrtJob) {
        let request = NrtRequest {
            cmd,
            index,
            client: from,
            job,
        };
        if self.nrt.submit(request).is_err() {
            self.fail(from, cmd, "NRT thread is down");
        } else {
            self.nrt_submitted += 1;
        }
    }

    /// `/buffer_query bufnum...` → `/buffer_query.reply` with (bufnum, frames, channels,
    /// sampleRate) per buffer; zeros for unallocated indices. Synchronous,
    /// answered from the mirror (= state as of the last completed command).
    fn handle_buffer_query(&mut self, msg: &OscMessage, from: ClientId) {
        // No argument lists every allocated buffer (M30): the patcher shows
        // real buffers as objects, and a client that never allocated them (the
        // pool outlives a session) has no other way to learn they exist.
        if msg.args.is_empty() {
            let args = self.translator.buffer_list();
            return self.reply(from, "/buffer_query.reply", args);
        }
        let mut args = Vec::with_capacity(msg.args.len() * 4);
        for arg in &msg.args {
            let OscType::Int(index) = arg else {
                return self.fail(from, "/buffer_query", "expected int buffer indices");
            };
            let info = self.mirror_buffer(*index);
            // An unallocated slot answers with `frames = -1`: absence is a
            // state reported in the record, like `/node_query`'s `isGroup = -1`
            // and `/def_query`'s empty family, so one dead index does not abort
            // the batch.
            args.push(OscType::Int(*index));
            args.push(OscType::Int(
                info.as_ref().map_or(-1, |b| b.frames() as i32),
            ));
            args.push(OscType::Int(
                info.as_ref().map_or(0, |b| b.channels() as i32),
            ));
            args.push(OscType::Float(
                info.as_ref().map_or(0.0, |b| b.sample_rate() as f32),
            ));
        }
        self.reply(from, "/buffer_query.reply", args);
    }

    /// `/def_query [name...]` → one `/def_query.reply` per def, then `/done "/def_query"`
    /// (M30). No argument lists every loaded def. The reply is one message per
    /// def because the control surface is variable-length: an aggregate would
    /// nest, and a large catalog would outgrow a UDP datagram.
    ///
    /// Retrieval only — the def store persists across sessions, so this is how
    /// a client learns what a running server actually holds.
    fn handle_def_query(&mut self, msg: &OscMessage, from: ClientId) {
        let mut names = Vec::with_capacity(msg.args.len());
        for arg in &msg.args {
            let OscType::String(name) = arg else {
                return self.fail(from, "/def_query", "expected string def names");
            };
            names.push(name.clone());
        }
        let requested = (!names.is_empty()).then_some(names.as_slice());
        for args in self.translator.def_info(requested) {
            self.reply(from, "/def_query.reply", args);
        }
        self.reply(from, "/done", vec![OscType::String("/def_query".into())]);
    }

    /// `/ugen_query [kind...]` → one `/ugen_query.reply` per UGen, then `/done "/ugen_query"`
    /// (M30): the catalog straight from the `dsp::registry` descriptors, so a
    /// palette derives from the server's truth instead of a client-side copy.
    /// An unknown kind replies with an empty rate set and no inputs.
    ///
    /// Faust primitives are deliberately absent: that vocabulary is Faust's
    /// own and already lives in the client builders.
    ///
    /// Built without the `synth` feature there is no UGen catalog at all, and
    /// the honest reply is an **empty** listing rather than a `/fail` — the
    /// same way `/def_query` on such a build simply lists no synth defs.
    fn handle_ugen_query(&mut self, msg: &OscMessage, from: ClientId) {
        let mut names = Vec::with_capacity(msg.args.len());
        for arg in &msg.args {
            let OscType::String(name) = arg else {
                return self.fail(from, "/ugen_query", "expected string UGen kinds");
            };
            names.push(name.clone());
        }
        #[cfg(feature = "synth")]
        for args in ugen_infos(&names) {
            self.reply(from, "/ugen_query.reply", args);
        }
        self.reply(from, "/done", vec![OscType::String("/ugen_query".into())]);
    }

    /// `/buffer_get bufnum index...` → `/buffer_get.reply bufnum index value...`: read single
    /// samples (flat, interleaved) from the buffer mirror. Out-of-range indices
    /// (and any index into an unallocated buffer) read as `0.0`, mirroring how
    /// `Buffer::sample` and the audio-rate UGens treat them. Synchronous, like
    /// `/buffer_query`.
    fn handle_buffer_get(&mut self, msg: &OscMessage, from: ClientId) {
        let Some((OscType::Int(bufnum), indices)) = msg.args.split_first() else {
            return self.fail(
                from,
                "/buffer_get",
                "expected bufnum then int sample indices",
            );
        };
        let buffer = self.mirror_buffer(*bufnum);
        let data = buffer.as_deref().map(|b| b.data()).unwrap_or(&[]);
        let mut args = vec![OscType::Int(*bufnum)];
        for arg in indices {
            let OscType::Int(index) = arg else {
                return self.fail(from, "/buffer_get", "expected int sample indices");
            };
            let value = usize::try_from(*index)
                .ok()
                .and_then(|i| data.get(i))
                .copied()
                .unwrap_or(0.0);
            args.push(OscType::Int(*index));
            args.push(OscType::Float(value));
        }
        self.reply(from, "/buffer_get.reply", args);
    }

    /// `/buffer_getRange bufnum [start count]...` → `/buffer_getRange.reply bufnum start count value...`:
    /// read ranges of samples (flat, interleaved) from the buffer mirror — the
    /// client-side counterpart of `/buffer_getRange.reply`, and how a GUI client pulls a buffer
    /// to display it. `count` is clamped to what the buffer holds from `start`,
    /// so a request past the end returns only the available samples (none for an
    /// unallocated buffer). Large buffers are read in client-chosen chunks,
    /// sized to the client's transport: a stream client (TCP/WS) may ask for
    /// up to the `/server_query` frame ceiling per reply, a UDP client must
    /// stay under the datagram cap. The shm bulk path is future work.
    fn handle_buffer_get_range(&mut self, msg: &OscMessage, from: ClientId) {
        let Some((OscType::Int(bufnum), pairs)) = msg.args.split_first() else {
            return self.fail(
                from,
                "/buffer_getRange",
                "expected bufnum then (start, count) pairs",
            );
        };
        if pairs.len() % 2 != 0 {
            return self.fail(from, "/buffer_getRange", "expected (start, count) pairs");
        }
        let buffer = self.mirror_buffer(*bufnum);
        let data = buffer.as_deref().map(|b| b.data()).unwrap_or(&[]);
        let mut args = vec![OscType::Int(*bufnum)];
        for pair in pairs.chunks_exact(2) {
            let (OscType::Int(start), OscType::Int(count)) = (&pair[0], &pair[1]) else {
                return self.fail(from, "/buffer_getRange", "expected int start and count");
            };
            let start = (*start).max(0) as usize;
            let count = (*count).max(0) as usize;
            let end = start.saturating_add(count).min(data.len());
            let slice = data.get(start..end).unwrap_or(&[]);
            args.push(OscType::Int(start as i32));
            args.push(OscType::Int(slice.len() as i32));
            args.extend(slice.iter().map(|s| OscType::Float(*s)));
        }
        self.reply(from, "/buffer_getRange.reply", args);
    }

    /// `/buffer_export bufnum path` → `/done /buffer_export bufnum`: write the buffer's raw
    /// samples (flat, interleaved, little-endian `f32`) to `path` as a **local
    /// shared resource**, so a same-machine client (the GUI host) can map and read
    /// a multi-megabyte buffer with no per-sample OSC traffic — the bulk-data path,
    /// the efficient counterpart of `/buffer_getRange`'s chunked over-the-wire reads. The
    /// reader pairs it with the buffer's channel count (from `/buffer_query`) to
    /// de-interleave. Synchronous on the network thread (not the audio thread),
    /// like `/buffer_get`/`/buffer_getRange`; replies `/fail` on a missing buffer or a write
    /// error.
    fn handle_buffer_export(&mut self, msg: &OscMessage, from: ClientId) {
        let (Some(OscType::Int(bufnum)), Some(OscType::String(path))) =
            (msg.args.first(), msg.args.get(1))
        else {
            return self.fail(from, "/buffer_export", "expected bufnum then a path string");
        };
        let Some(buffer) = self.mirror_buffer(*bufnum) else {
            return self.fail(from, "/buffer_export", "no such buffer");
        };
        let mut bytes = Vec::with_capacity(buffer.data().len() * 4);
        for &s in buffer.data() {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        match std::fs::write(path, &bytes) {
            Ok(()) => self.reply(
                from,
                "/done",
                vec![
                    OscType::String("/buffer_export".into()),
                    OscType::Int(*bufnum),
                ],
            ),
            Err(e) => self.fail(from, "/buffer_export", format!("write {path}: {e}")),
        }
    }

    fn mirror_buffer(&self, index: i32) -> Option<Arc<crate::dsp::buffer::Buffer>> {
        usize::try_from(index)
            .ok()
            .and_then(|i| self.translator.buffers.get(i))
            .and_then(|b| b.as_ref().map(Arc::clone))
    }

    fn handle_server_notify(&mut self, msg: &OscMessage, from: ClientId) {
        match msg.args.first() {
            Some(OscType::Int(1)) => {
                let id = match self.clients.iter().position(|c| *c == from) {
                    Some(i) => i + 1,
                    None => {
                        self.clients.push(from);
                        self.clients.len()
                    }
                };
                self.reply(
                    from,
                    "/done",
                    vec![
                        OscType::String("/server_notify".into()),
                        OscType::Int(id as i32),
                    ],
                );
            }
            Some(OscType::Int(0)) => {
                self.clients.retain(|c| *c != from);
                self.reply(
                    from,
                    "/done",
                    vec![OscType::String("/server_notify".into())],
                );
            }
            _ => self.fail(from, "/server_notify", "expected int argument 0 or 1"),
        }
    }

    fn fail(&self, to: ClientId, cmd: &str, why: impl Into<String>) {
        let why = why.into();
        // The console post is gated by `/server_errorMode`; the OSC `/fail` reply always
        // goes out (clients rely on it).
        if self.post_errors {
            warn!(target: crate::logging::OSC_TARGET, "FAILURE {cmd}: {why}");
        }
        self.reply(
            to,
            "/fail",
            vec![OscType::String(cmd.into()), OscType::String(why)],
        );
    }

    fn reply(&self, to: ClientId, addr: &str, args: Vec<OscType>) {
        let packet = OscPacket::Message(OscMessage {
            addr: addr.into(),
            args,
        });
        let bytes = match encoder::encode(&packet) {
            Ok(bytes) => bytes,
            Err(e) => return warn!("failed to encode {addr}: {e}"),
        };
        match to {
            ClientId::Udp(addr_to) => {
                // A headless server has no UDP clients; dropped if so.
                if let Some(socket) = &self.socket
                    && let Err(e) = socket.send_to(&bytes, addr_to)
                {
                    warn!("failed to send {addr} to {addr_to}: {e}");
                }
            }
            ClientId::Tcp(id) => {
                // Length-prefixed reply on the originating connection; dropped
                // if it has since closed.
                if let Some(hub) = &self.tcp {
                    hub.reply(id, &bytes);
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            ClientId::Ws(id) => {
                // Binary-message reply on the originating connection; dropped if
                // it has since closed.
                if let Some(hub) = &self.ws {
                    hub.reply(id, &bytes);
                }
            }
            // No WebSocket hub exists on wasm32; the variant is unreachable.
            #[cfg(target_arch = "wasm32")]
            ClientId::Ws(_) => {}
            ClientId::Ring => {
                // Backpressure, not loss: a full reply ring means the client
                // stopped draining; dropping the reply is all we can do
                // without blocking the server.
                if let Some(ipc) = &self.ipc
                    && !ipc.push(&bytes)
                {
                    warn!("reply ring full: dropping {addr}");
                }
            }
        }
    }
}

/// The raw `SynthDefSpec` JSON of a `/def_send synth` message (blob or string form),
/// for persisting it verbatim. Mirrors the argument parsing in
/// [`CmdTranslator::d_recv`].
/// The `/ugen_query.reply` argument vectors for a `/ugen_query` (M30): the whole catalog
/// when `names` is empty, otherwise one per requested kind — an unknown one
/// coming back with an empty rate set and no inputs, so a batch never fails
/// wholesale (the `/buffer_query` convention).
#[cfg(feature = "synth")]
fn ugen_infos(names: &[String]) -> Vec<Vec<OscType>> {
    if names.is_empty() {
        return crate::dsp::registry::all().iter().map(ugen_info).collect();
    }
    names
        .iter()
        .map(|name| match crate::dsp::registry::lookup(name) {
            Some(d) => ugen_info(d),
            None => vec![
                OscType::String(name.clone()),
                OscType::Int(0),
                OscType::String(String::new()),
                OscType::String(String::new()),
                OscType::String(String::new()),
                OscType::String(String::new()),
                OscType::Int(0),
                OscType::String(String::new()),
                OscType::String(String::new()),
                OscType::Int(0),
            ],
        })
        .collect()
}

/// One `/ugen_query.reply` argument vector from a catalog descriptor (M30).
///
/// Layout: `name, arity, defaultRate, rates, exec, bus, needsPath, opFamily,
/// spectral, numInputs` then per input `name, default`. `arity` is `-1` for a
/// variadic kind, whose named inputs are its fixed head only. The enum-valued
/// fields are lowercase names, `""` for the "not applicable" variant.
#[cfg(feature = "synth")]
fn ugen_info(d: &crate::dsp::registry::UGenDescriptor) -> Vec<OscType> {
    use crate::dsp::registry::{Arity, BusRole, ExecMode, OpFamily, SpectralRole};
    let rates: Vec<&str> = d.rates.iter().map(|r| r.as_str()).collect();
    let mut args = vec![
        OscType::String(d.name.into()),
        OscType::Int(match d.arity {
            Arity::Fixed(n) => n as i32,
            Arity::Variadic => -1,
        }),
        OscType::String(d.default_rate.as_str().into()),
        OscType::String(rates.join(",")),
        OscType::String(
            match d.exec {
                ExecMode::Normal => "normal",
                ExecMode::LocalIn => "local_in",
                ExecMode::LocalOut => "local_out",
                ExecMode::DemandDriver => "demand_driver",
                ExecMode::DoneQuery => "done_query",
                ExecMode::Spectral => "spectral",
            }
            .into(),
        ),
        OscType::String(
            match d.bus {
                BusRole::None => "",
                BusRole::Read => "read",
                BusRole::Write => "write",
                BusRole::ReadWrite => "read_write",
            }
            .into(),
        ),
        OscType::Int(d.needs_path as i32),
        OscType::String(
            match d.op_family {
                None => "",
                Some(OpFamily::Unary) => "unary",
                Some(OpFamily::Binary) => "binary",
            }
            .into(),
        ),
        OscType::String(
            match d.spectral {
                SpectralRole::None => "",
                SpectralRole::Source => "source",
                SpectralRole::Filter => "filter",
                SpectralRole::Filter2 => "filter2",
                SpectralRole::Sink => "sink",
            }
            .into(),
        ),
        OscType::Int(d.inputs.len() as i32),
    ];
    for i in d.inputs {
        args.push(OscType::String(i.name.into()));
        args.push(OscType::Float(i.default));
    }
    args
}

fn synthdef_spec_bytes(args: &[OscType]) -> Option<&[u8]> {
    match args.first() {
        Some(OscType::Blob(b)) => Some(b),
        Some(OscType::String(s)) => Some(s.as_bytes()),
        _ => None,
    }
}

/// Seconds between the NTP epoch (1900) and the Unix epoch (1970).
const NTP_UNIX_OFFSET: f64 = 2_208_988_800.0;

/// The current wall-clock instant as an OSC/NTP timetag (seconds since 1900 in
/// a 32-bit count, plus a 32-bit binary fraction) — the inverse of the NTP→Unix
/// math in [`timetag_delta_secs`]. Published alongside the sample counter in
/// `/clock_query.reply` so a client gets the anchor `(osc_time, sample)` it needs to
/// place its logical OSC time on this server's sample axis.
fn unix_to_ntp(unix: f64) -> OscTime {
    let ntp = unix + NTP_UNIX_OFFSET;
    let seconds = ntp.trunc();
    OscTime {
        seconds: seconds as u32,
        fractional: ((ntp - seconds) * 2f64.powi(32)) as u32,
    }
}

impl OscServer {
    /// The server's current time as an OSC/NTP timetag, from its
    /// [`TimeSource`] (wall natively, the anchored sample axis headless).
    fn now_ntp(&self) -> OscTime {
        unix_to_ntp(self.unix_secs())
    }

    /// Seconds from now until the timetag fires. `None` is the OSC
    /// "immediately" tag (seconds 0, fractional 1 — rosc keeps it verbatim).
    fn timetag_delta_secs(&self, t: OscTime) -> Option<f64> {
        if t.seconds == 0 && t.fractional <= 1 {
            return None;
        }
        let target = t.seconds as f64 - NTP_UNIX_OFFSET + t.fractional as f64 / 2f64.powi(32);
        Some(target - self.unix_secs())
    }
}
