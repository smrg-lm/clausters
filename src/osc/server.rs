//! UDP OSC server implementing the M5 subset of the scsynth protocol:
//! `/status`, `/quit`, `/notify`, `/dumpOSC`, `/verbosity`, `/s_new` (add actions 0-4),
//! `/n_free`, `/n_set`, `/n_before`, `/n_after`, `/g_new`, `/g_freeAll`,
//! `/g_deepFree`, `/c_set`, `/c_get`, `/d_recv`, `/d_free`; the buffer
//! commands `/b_alloc`, `/b_allocRead`, `/b_read`, `/b_write`, `/b_zero`,
//! `/b_free` (all async via the NRT thread, replying `/done cmd bufnum`),
//! `/b_query` (synchronous `/b_info`), the synchronous reads `/b_get`
//! (`/b_set`) and `/b_getn` (`/b_setn`), and `/b_export` (dump raw samples to a
//! local file for the shared-resource bulk path); `/n_go` and
//! `/n_end` notifications go to `/notify` clients. With the `faust` feature,
//! `/d_faust name def` compiles a def — JSON box graph (F2) or raw Faust
//! source (F1) — on the dedicated compiler thread and replies
//! `/done`/`/fail` asynchronously; `/s_new` instantiates Faust defs like any
//! other (F3), with the def's UI parameters plus the reserved `out`/`in` bus
//! controls as `/n_set` names.
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
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rosc::{OscBundle, OscMessage, OscPacket, OscTime, OscType, encoder};
use tracing::{error, warn};

#[cfg(feature = "faust")]
use crate::faust::compiler::{CacheJob, CompilePayload, CompileRequest, CompilerThread};
use crate::osc::ClientId;
use crate::osc::translate::{CmdTranslator, float_value, int_arg, parse_b_gen, parse_buffer_msg};
use crate::server::defstore::DefStore;
use crate::server::engine::{Cmd, EngineHandle, Garbage, NodeEventKind};
use crate::server::nrt::{NrtAction, NrtJob, NrtRequest, NrtThread};

/// Default scsynth port.
pub const DEFAULT_PORT: u16 = 57110;

/// Largest UDP datagram we accept.
const RECV_BUF_SIZE: usize = 65536;

/// How long `recv_from` blocks before we take a garbage-collection pass.
const GC_INTERVAL: Duration = Duration::from_millis(100);

/// Information reported in `/status.reply` that does not come from the
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
    socket: UdpSocket,
    info: ServerInfo,
    handle: EngineHandle,
    /// Def tables, node→def mirror and message→command translation, shared
    /// with the NRT renderer (see [`crate::osc::translate`]).
    /// Owns the network-side buffer mirror (`translator.buffers`), updated
    /// when NRT results are installed: serves `/b_query` and gives `/b_read`,
    /// `/b_write` and `/b_zero` the current contents/shape, and a Faust
    /// instance its `soundfile` data.
    translator: CmdTranslator,
    nrt: NrtThread,
    /// Clients registered via `/notify 1`; the client ID is index + 1.
    clients: Vec<ClientId>,
    recv_buf: Vec<u8>,
    /// M14: the shared-memory / in-process ring endpoint, when attached.
    ipc: Option<crate::server::ipc::IpcPeer>,
    /// TCP transport, when `listen_tcp` was called: accepts length-prefixed OSC
    /// connections multiplexed into the same loop. See [`crate::osc::tcp`].
    tcp: Option<crate::osc::tcp::TcpHub>,
    /// WebSocket transport, when `listen_ws` was called: the same OSC encoding
    /// over WebSocket binary messages, reachable from a browser. Multiplexed
    /// into the same loop as TCP. See [`crate::osc::ws`].
    ws: Option<crate::osc::ws::WsHub>,
    /// M17 live MIDI input, when `listen_midi` was called: a virtual ALSA port
    /// whose decoded messages the loop drains. See [`crate::midi::live`].
    #[cfg(feature = "midi")]
    midi: Option<crate::midi::live::MidiHub>,
    /// On-disk def persistence, when a data directory is configured. Defs
    /// loaded from it on startup; `/d_recv`/`/d_faust` write to it,
    /// `/d_free` deletes from it.
    store: Option<DefStore>,
    /// The compiler thread is owned here and dies with the server.
    #[cfg(feature = "faust")]
    faust_compiler: CompilerThread,
    /// `/sync` barrier bookkeeping. Each async pipeline (NRT buffers, Faust
    /// compiles) completes FIFO on its own thread, so a monotonic
    /// submitted/drained counter per pipeline is enough: a `/sync` records the
    /// current submitted counts as its targets and is answered with `/synced`
    /// once both drained counts have caught up. See [`Self::handle_sync`].
    nrt_submitted: u64,
    nrt_drained: u64,
    faust_submitted: u64,
    faust_drained: u64,
    pending_syncs: Vec<PendingSync>,
    /// The shared beat grid (`/transport`), once a client defines one.
    transport: Option<Transport>,
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

/// A `/sync` waiting for the async pipelines to drain up to its targets.
struct PendingSync {
    client: ClientId,
    id: i32,
    nrt_target: u64,
    faust_target: u64,
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
        let translator = CmdTranslator::with_buses(
            handle.sample_rate,
            handle.audio_buses,
            handle.control_buses().len(),
        );
        Ok(Self {
            socket,
            info,
            handle,
            translator,
            nrt: NrtThread::spawn(),
            clients: Vec::new(),
            ipc: None,
            tcp: None,
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
        })
    }

    /// Enables on-disk persistence and reloads whatever defs the store
    /// already holds. SynthDefs are recompiled inline (cheap); Faust defs are
    /// queued on the compiler thread, restoring from the bitcode cache when
    /// possible, so the socket starts serving immediately and the library
    /// loads incrementally.
    pub fn attach_store(&mut self, store: DefStore) {
        for spec in store.load_synthdef_specs() {
            if let Err(e) = self.translator.d_recv(&[OscType::Blob(spec)]) {
                warn!("persisted SynthDef failed to load: {e}");
            }
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
        self.socket.local_addr()
    }

    /// Starts accepting length-prefixed OSC over TCP on `addr` (server track M /
    /// client C8). The run loop drains the connections every iteration and a
    /// zero-length UDP datagram to our own address wakes it the moment a frame
    /// arrives, so TCP requests don't wait for the GC tick. Returns the bound
    /// TCP address.
    pub fn listen_tcp(&mut self, addr: impl ToSocketAddrs) -> io::Result<SocketAddr> {
        // Reader threads wake the loop by pinging the UDP socket; if we bound to
        // an unspecified address, ping loopback on the same port.
        let mut wake_target = self.socket.local_addr()?;
        if wake_target.ip().is_unspecified() {
            wake_target.set_ip(match wake_target {
                SocketAddr::V4(_) => std::net::Ipv4Addr::LOCALHOST.into(),
                SocketAddr::V6(_) => std::net::Ipv6Addr::LOCALHOST.into(),
            });
        }
        let hub = crate::osc::tcp::TcpHub::bind(addr, wake_target)?;
        let bound = hub.local_addr();
        self.tcp = Some(hub);
        Ok(bound)
    }

    /// Starts accepting OSC over WebSocket on `addr`. Same loop multiplexing and
    /// zero-length-UDP wake as [`Self::listen_tcp`]: the run loop drains
    /// WebSocket frames every iteration and a connection thread pings our UDP
    /// socket the moment a frame arrives. Returns the bound address; connect a
    /// browser with `ws://<addr>/`.
    pub fn listen_ws(&mut self, addr: impl ToSocketAddrs) -> io::Result<SocketAddr> {
        let mut wake_target = self.socket.local_addr()?;
        if wake_target.ip().is_unspecified() {
            wake_target.set_ip(match wake_target {
                SocketAddr::V4(_) => std::net::Ipv4Addr::LOCALHOST.into(),
                SocketAddr::V6(_) => std::net::Ipv6Addr::LOCALHOST.into(),
            });
        }
        let hub = crate::osc::ws::WsHub::bind(addr, wake_target)?;
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
        let mut wake_target = self.socket.local_addr()?;
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
        self.socket
            .set_read_timeout(Some(Duration::from_millis(2)))?;
        self.ipc = Some(peer);
        Ok(())
    }

    /// Blocks serving requests until a `/quit` arrives.
    pub fn run(&mut self) -> io::Result<()> {
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
            let (len, from) = match self.socket.recv_from(&mut self.recv_buf) {
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

    /// Handles every complete WebSocket frame currently queued. Same validation
    /// path as UDP (`decode_packet`); WebSocket bytes are untrusted. Replies
    /// route back to the originating connection via [`ClientId::Ws`].
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
    /// reuses the `/s_new`/`/n_set`/`/n_free` path and keeps the tree mirror in
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
                                OscType::String("/d_faust".into()),
                                OscType::String(result.name),
                            ],
                        );
                    }
                }
                Err(error) => match result.client {
                    Some(client) => self.fail(client, "/d_faust", error),
                    None => warn!("persisted Faust def '{}' failed: {error}", result.name),
                },
            }
        }
        self.resolve_syncs();
    }

    /// `/d_faust name def`: queue an async Faust compilation. The def format
    /// is sniffed by [`CompilePayload::classify`]: raw Faust source (F1), a
    /// JSON box graph (F2, `faust::boxes`), or a JSON signal tree
    /// (`faust::signals`, root `{"signals": …}`).
    #[cfg(feature = "faust")]
    fn handle_d_faust(&mut self, msg: &OscMessage, from: ClientId) {
        let (name, def) = match crate::osc::translate::parse_d_faust(&msg.args) {
            Ok(pair) => pair,
            Err(e) => return self.fail(from, "/d_faust", e),
        };
        let payload = CompilePayload::classify(def);
        // A live /d_faust always compiles fresh from the given def and, with
        // persistence on, (re)writes the cache (restore = None).
        let cache = self.store.as_ref().map(|s| {
            Box::new(CacheJob {
                dir: s.faustdefs_dir().to_path_buf(),
                restore: None,
            })
        });
        let request = CompileRequest {
            name,
            payload,
            client: Some(from),
            cache,
        };
        if self.faust_compiler.submit(request).is_err() {
            self.fail(from, "/d_faust", "compiler thread is down");
        } else {
            self.faust_submitted += 1;
        }
    }

    #[cfg(not(feature = "faust"))]
    fn handle_d_faust(&mut self, _msg: &OscMessage, from: ClientId) {
        self.fail(from, "/d_faust", "server built without faust support");
    }

    /// `/sync id`: the async barrier (scsynth semantics). Records the current
    /// submitted counts as targets and is answered with `/synced id` once both
    /// async pipelines (NRT buffers, Faust compiles) have drained up to them —
    /// i.e. every async command received before this `/sync` has finished.
    /// Each pipeline completes FIFO, so the counters are a sufficient barrier.
    fn handle_sync(&mut self, msg: &OscMessage, from: ClientId) {
        let id = match msg.args.first() {
            Some(OscType::Int(id)) => *id,
            _ => return self.fail(from, "/sync", "expected an int id"),
        };
        self.pending_syncs.push(PendingSync {
            client: from,
            id,
            nrt_target: self.nrt_submitted,
            faust_target: self.faust_submitted,
        });
        self.resolve_syncs(); // answer at once if nothing is outstanding
    }

    /// Answers every pending `/sync` whose target counts have been reached.
    /// Called after each async drain (and from [`Self::handle_sync`]).
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
            self.reply(client, "/synced", vec![OscType::Int(id)]);
        }
    }

    /// Drops what the audio thread discarded, keeps the def mirror in sync
    /// and forwards node lifecycle events to `/notify` clients.
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
                    // original node is still alive under this ID.
                    warn!("engine rejected node {id} (duplicate ID, bad target or full table)");
                }
            }
        }
        while let Some(ev) = self.handle.pop_event() {
            let addr = match ev.kind {
                NodeEventKind::Go => "/n_go",
                NodeEventKind::End => "/n_end",
            };
            // scsynth shape: id, parent, previous, next, isGroup. We don't
            // track sibling IDs on this side, so previous/next are -1.
            let args = vec![
                OscType::Int(ev.id),
                OscType::Int(ev.parent_id),
                OscType::Int(-1),
                OscType::Int(-1),
                OscType::Int(ev.is_group as i32),
            ];
            for client in &self.clients {
                self.reply(*client, addr, args.clone());
            }
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
        match timetag_delta_secs(bundle.timetag) {
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
            "/status" => self.send_status(from),
            "/server_info" => self.send_server_info(from),
            "/notify" => self.handle_notify(&msg, from),
            // The translator covers the whole schedulable subset (and keeps
            // the M12 tree mirror in sync), so the immediate forms share one
            // path: translate, then ship every command.
            "/s_new" | "/g_new" | "/g_freeAll" | "/g_deepFree" | "/n_free" | "/n_run"
            | "/n_set" | "/n_map" | "/n_mapa" | "/n_before" | "/n_after" | "/g_sortMode"
            | "/g_parallel" | "/graph_new" | "/graph_voice" => {
                self.handle_via_translate(&msg, from)
            }
            // MIDI binding mutations also persist the binding set (M19).
            "/midi_bind" | "/midi_unbind" | "/midi_map" => {
                self.handle_via_translate(&msg, from);
                self.persist_bindings();
            }
            "/g_queryTree" => self.handle_g_query_tree(&msg, from),
            "/n_query" => self.handle_n_query(&msg, from),
            "/g_dumpGraph" => self.handle_g_dump_graph(&msg, from),
            "/c_set" => self.handle_c_set(&msg, from),
            "/c_get" => self.handle_c_get(&msg, from),
            "/clock" => self.handle_clock(from),
            "/sched" => self.handle_sched(&msg, from),
            "/b_alloc" => self.handle_b_cmd(&msg, from, "/b_alloc"),
            "/b_allocRead" => self.handle_b_cmd(&msg, from, "/b_allocRead"),
            "/b_read" => self.handle_b_cmd(&msg, from, "/b_read"),
            "/b_write" => self.handle_b_cmd(&msg, from, "/b_write"),
            "/b_zero" => self.handle_b_cmd(&msg, from, "/b_zero"),
            "/b_gen" => self.handle_b_gen(&msg, from),
            "/b_free" => self.handle_b_cmd(&msg, from, "/b_free"),
            "/b_query" => self.handle_b_query(&msg, from),
            "/b_get" => self.handle_b_get(&msg, from),
            "/b_getn" => self.handle_b_getn(&msg, from),
            "/b_export" => self.handle_b_export(&msg, from),
            "/sync" => self.handle_sync(&msg, from),
            "/d_recv" => self.handle_d_recv(&msg, from),
            "/d_faust" => self.handle_d_faust(&msg, from),
            "/d_graph" => self.handle_d_graph(&msg, from),
            "/d_free" => self.handle_d_free(&msg, from),
            "/dumpOSC" => self.handle_dump_osc(&msg, from),
            "/verbosity" => self.handle_verbosity(&msg, from),
            "/transport" => self.handle_transport(&msg, from),
            "/transport_play" => self.handle_transport_play(&msg, from),
            "/transport_stop" => self.handle_transport_stop(from),
            "/transport_locate" => self.handle_transport_locate(&msg, from),
            "/quit" => {
                self.reply(from, "/done", vec![OscType::String("/quit".into())]);
                return Flow::Quit;
            }
            other => self.fail(from, other, "unknown command"),
        }
        Flow::Continue
    }

    /// `/dumpOSC flag`: toggles the OSC-traffic log overlay (the `clausters::osc`
    /// trace target). Unlike scsynth's console dump, this routes through the
    /// logging system the client also controls with `/verbosity`; output is on
    /// the server's stderr. Replies `/done`.
    fn handle_dump_osc(&mut self, msg: &OscMessage, from: ClientId) {
        let on = matches!(msg.args.first(), Some(OscType::Int(n)) if *n != 0);
        match crate::logging::set_osc_dump(on) {
            Ok(()) => self.reply(from, "/done", vec![OscType::String("/dumpOSC".into())]),
            Err(e) => self.fail(from, "/dumpOSC", e),
        }
    }

    /// `/verbosity level`: the client retunes the server's log level live.
    /// `level` is an int (`-1` errors, `0` warn, `1` info, `2` debug, `3+`
    /// trace) or a string `EnvFilter` directive (e.g. `"clausters::osc=trace"`).
    /// Replies `/done`. (Uncommon, but it lets a client steer server logs
    /// without restarting; the initial level comes from `-v`/`RUST_LOG`.)
    fn handle_verbosity(&mut self, msg: &OscMessage, from: ClientId) {
        let result = match msg.args.first() {
            Some(OscType::Int(n)) => crate::logging::set_verbosity(*n as i8),
            Some(OscType::String(s)) => crate::logging::set_base(s),
            _ => Err("expected an int level or a string filter directive".to_string()),
        };
        match result {
            Ok(()) => self.reply(from, "/done", vec![OscType::String("/verbosity".into())]),
            Err(e) => self.fail(from, "/verbosity", e),
        }
    }

    fn send_status(&mut self, to: ClientId) {
        let counters = self.handle.counters();
        let num_defs = self.translator.def_count();
        let args = vec![
            OscType::Int(1),
            OscType::Int(counters.ugens.load(Ordering::Relaxed) as i32),
            OscType::Int(counters.synths.load(Ordering::Relaxed) as i32),
            OscType::Int(counters.groups.load(Ordering::Relaxed) as i32),
            OscType::Int(num_defs as i32),
            OscType::Float(0.0), // avg CPU
            OscType::Float(0.0), // peak CPU
            OscType::Double(self.info.nominal_sample_rate),
            OscType::Double(self.info.actual_sample_rate),
        ];
        self.reply(to, "/status.reply", args);
    }

    /// Reports the server's static configuration so a client can size its own
    /// bus/allocator state from the server instead of hardcoding it:
    /// `/server_info.reply [audio_buses, control_buses, output_channels,
    /// block_size, nominal_sr, actual_sr]`.
    fn send_server_info(&mut self, to: ClientId) {
        let args = vec![
            OscType::Int(self.handle.audio_buses as i32),
            OscType::Int(self.handle.control_buses().len() as i32),
            OscType::Int(self.handle.channels as i32),
            OscType::Int(crate::dsp::BLOCK_SIZE as i32),
            OscType::Double(self.info.nominal_sample_rate),
            OscType::Double(self.info.actual_sample_rate),
        ];
        self.reply(to, "/server_info.reply", args);
    }

    /// M8: the sample-clock query. Replies `/clock.reply` with the engine's
    /// sample counter (int64 `h`), the actual sample rate (double `d`) and the
    /// server's OSC/NTP time captured with the counter (timetag `t`). The
    /// `(osc_time, sample)` pair is the master-clock **anchor**: a client maps
    /// its logical OSC time `T` to this server's sample axis with
    /// `S0 + (T − T0)·rate` and schedules with [`/sched`] (`Self::handle_sched`)
    /// directly in samples — see `docs/sample-clock.md`. Clients that only want
    /// the older two-field form ignore the trailing timetag. The counter counts
    /// *processed* samples: it runs a device buffer ahead of the speakers and
    /// pauses on xruns.
    fn handle_clock(&mut self, from: ClientId) {
        // Read the counter and the wall clock back-to-back so the published
        // anchor pairs the same instant (the sub-microsecond gap is negligible).
        let sample = self.handle.current_samples();
        let args = vec![
            OscType::Long(sample as i64),
            OscType::Double(self.info.actual_sample_rate),
            OscType::Time(now_ntp()),
        ];
        self.reply(from, "/clock.reply", args);
    }

    /// The `/transport.reply` payload: the grid plus the rolling state,
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

    /// Pushes the current transport state to every `/notify` client, so a
    /// responder on `/transport.reply` re-aligns or rolls its playhead live when
    /// the conductor changes the grid, plays, stops or locates — no polling.
    fn broadcast_transport(&self) {
        let push = self.transport_reply_args();
        for client in &self.clients {
            self.reply(*client, "/transport.reply", push.clone());
        }
    }

    /// `/transport` — the shared beat grid for phase-aligning several clients on
    /// the master sample clock. **No args queries** it; replies
    /// `/transport.reply (origin_sample:int64, tempo:double, defined:int32,
    /// playing:int32, position:double)`, all zeros (and `defined` 0) when none is
    /// set. Two args `(origin_sample:int64, tempo:double)` **set** the grid (last
    /// writer wins), stopped at position 0, and reply `/done`. The grid is `beat
    /// b -> sample origin_sample + b·rate/tempo`; a client joins by reading it
    /// and quantizing its start onto it. The server only stores/broadcasts it —
    /// in-memory (resets on restart), never scheduling audio from it.
    ///
    /// The rolling state (play/stop/locate) rides on top: see
    /// [`Self::handle_transport_play`]. Any change is **pushed** to every
    /// `/notify` client (the C13 responder path).
    fn handle_transport(&mut self, msg: &OscMessage, from: ClientId) {
        if msg.args.is_empty() {
            let args = self.transport_reply_args();
            self.reply(from, "/transport.reply", args);
            return;
        }
        let origin = match msg.args.first() {
            Some(OscType::Long(v)) => *v,
            Some(OscType::Int(v)) => *v as i64,
            _ => {
                return self.fail(
                    from,
                    "/transport",
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
                    "/transport",
                    "expected (int64 originSample, double tempo)",
                );
            }
        };
        if origin < 0 || !(tempo > 0.0) {
            return self.fail(
                from,
                "/transport",
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
        self.reply(from, "/done", vec![OscType::String("/transport".into())]);
        self.broadcast_transport();
    }

    /// `/transport_play [position:double]` — start the transport rolling. With a
    /// `position` argument, playback starts from that song-position beat;
    /// without one, from where it last stopped/located. Every client's playhead
    /// obeys the broadcast (starting from `position`, quantized to the shared
    /// grid). Needs a grid defined first (`/transport`).
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

    /// M8: `/sched <int64 target> <blob packet>` — a timed bundle whose time
    /// is an absolute position on the **sample clock** instead of an NTP
    /// timetag (the OSC timetag format is NTP by spec, so sample targets get
    /// a container message rather than a reinterpreted tag; both front-ends
    /// feed the same engine queue and coexist freely). The blob is a complete
    /// OSC packet; all its leaf messages execute atomically at the target
    /// sample — nested bundle timetags inside the blob are **ignored**, one
    /// `/sched` is one instant. Past targets run at the start of the next
    /// block, like late NTP bundles.
    fn handle_sched(&mut self, msg: &OscMessage, from: ClientId) {
        let target = match msg.args.first() {
            Some(OscType::Long(t)) => *t,
            // Tolerated for hand-written clients; real targets outgrow i32
            // in under 13 hours at 48 kHz.
            Some(OscType::Int(t)) => *t as i64,
            _ => return self.fail(from, "/sched", "expected (int64 sampleTarget, blob packet)"),
        };
        if target < 0 {
            return self.fail(from, "/sched", "sample target must be >= 0");
        }
        let Some(OscType::Blob(blob)) = msg.args.get(1) else {
            return self.fail(from, "/sched", "expected (int64 sampleTarget, blob packet)");
        };
        let packet = match crate::osc::decode_packet(blob) {
            Ok(packet) => packet,
            Err(e) => return self.fail(from, "/sched", format!("bad packet blob: {e}")),
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
            self.fail(from, "/sched", "command FIFO full");
        }
    }

    /// Translates every leaf message of a `/sched` blob, like
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

    fn handle_d_recv(&mut self, msg: &OscMessage, from: ClientId) {
        match self.translator.d_recv(&msg.args) {
            Ok(name) => {
                if let Some(store) = &self.store
                    && let Some(spec) = synthdef_spec_bytes(&msg.args)
                    && let Err(e) = store.save_synthdef(&name, spec)
                {
                    error!("could not persist SynthDef '{name}': {e}");
                }
                self.reply(from, "/done", vec![OscType::String("/d_recv".into())]);
            }
            Err(e) => self.fail(from, "/d_recv", e),
        }
    }

    /// `/d_graph <json>` (M18): load a GraphDef (validate + store), persist its
    /// spec verbatim, and reply `/done`. Cheap — no JIT, just validation.
    fn handle_d_graph(&mut self, msg: &OscMessage, from: ClientId) {
        match self.translator.d_graph(&msg.args) {
            Ok(name) => {
                if let Some(store) = &self.store
                    && let Some(spec) = synthdef_spec_bytes(&msg.args)
                    && let Err(e) = store.save_graphdef(&name, spec)
                {
                    error!("could not persist GraphDef '{name}': {e}");
                }
                self.reply(from, "/done", vec![OscType::String("/d_graph".into())]);
            }
            Err(e) => self.fail(from, "/d_graph", e),
        }
    }

    fn handle_d_free(&mut self, msg: &OscMessage, from: ClientId) {
        if let Err(e) = self.translator.d_free(&msg.args) {
            return self.fail(from, "/d_free", e);
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

    /// M12: the node tree as seen by the network-side mirror, in scsynth's
    /// `/g_queryTree.reply` format. Args: [groupID = 0, flag = 0]; flag 1
    /// includes control names and values.
    fn handle_g_query_tree(&mut self, msg: &OscMessage, from: ClientId) {
        let group = int_arg(&msg.args, 0).unwrap_or(0);
        let flag = int_arg(&msg.args, 1).unwrap_or(0);
        match self.translator.query_tree(group, flag != 0) {
            Ok(args) => self.reply(from, "/g_queryTree.reply", args),
            Err(e) => self.fail(from, "/g_queryTree", e),
        }
    }

    /// Per-node detail: replies `/n_info` for each queried node ID (scsynth's
    /// `/n_query`, extended with the def name, controls, maps and inferred
    /// bus usage — see [`CmdTranslator::node_info`]).
    fn handle_n_query(&mut self, msg: &OscMessage, from: ClientId) {
        for arg in &msg.args {
            let OscType::Int(id) = arg else {
                return self.fail(from, "/n_query", "expected int node ids");
            };
            match self.translator.node_info(*id) {
                Ok(args) => self.reply(from, "/n_info", args),
                Err(e) => self.fail(from, "/n_query", e),
            }
        }
    }

    /// M12 debug: the inferred bus graph of one group as a string reply.
    fn handle_g_dump_graph(&mut self, msg: &OscMessage, from: ClientId) {
        let group = int_arg(&msg.args, 0).unwrap_or(0);
        match self.translator.dump_graph(group) {
            Ok(dump) => self.reply(
                from,
                "/g_dumpGraph.reply",
                vec![OscType::Int(group), OscType::String(dump)],
            ),
            Err(e) => self.fail(from, "/g_dumpGraph", e),
        }
    }

    /// Control buses are shared atomics: set directly, no engine round-trip.
    fn handle_c_set(&mut self, msg: &OscMessage, from: ClientId) {
        if msg.args.is_empty() || !msg.args.len().is_multiple_of(2) {
            return self.fail(from, "/c_set", "expected (busIndex, value) pairs");
        }
        for pair in msg.args.chunks(2) {
            let (OscType::Int(index), Some(value)) = (&pair[0], float_value(&pair[1])) else {
                return self.fail(from, "/c_set", "expected int bus index and number value");
            };
            if *index < 0 {
                return self.fail(from, "/c_set", "bus index must be non-negative");
            }
            self.handle.control_buses().set(*index as usize, value);
        }
    }

    /// Replies with a `/c_set` message carrying (busIndex, value) pairs.
    fn handle_c_get(&mut self, msg: &OscMessage, from: ClientId) {
        let mut args = Vec::with_capacity(msg.args.len() * 2);
        for arg in &msg.args {
            let OscType::Int(index) = arg else {
                return self.fail(from, "/c_get", "expected int bus indices");
            };
            if *index < 0 {
                return self.fail(from, "/c_get", "bus index must be non-negative");
            }
            args.push(OscType::Int(*index));
            args.push(OscType::Float(
                self.handle.control_buses().get(*index as usize),
            ));
        }
        self.reply(from, "/c_set", args);
    }

    /// Drains finished NRT jobs: installs/clears buffers in the engine and
    /// the mirror, and sends the async `/done cmd bufnum` / `/fail` replies.
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

    /// Any of the async `/b_*` commands: parsing is shared with the NRT
    /// renderer; the job runs on the NRT thread. `/b_free` also travels
    /// through the queue so it cannot overtake a pending alloc/read on the
    /// same index.
    fn handle_b_cmd(&mut self, msg: &OscMessage, from: ClientId, cmd: &'static str) {
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

    /// `/b_gen bufnum cmd ...`: fills a buffer through the wavetable/generator
    /// path (see [`parse_b_gen`]). Async on the NRT queue, in submission order
    /// with the other `/b_*` commands, replying `/done`/`/fail` like them.
    fn handle_b_gen(&mut self, msg: &OscMessage, from: ClientId) {
        let (index, job) = match parse_b_gen(&msg.args, &self.translator.buffers) {
            Ok(parsed) => parsed,
            Err(e) => return self.fail(from, "/b_gen", e),
        };
        self.submit_nrt("/b_gen", index, from, job);
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

    /// `/b_query bufnum...` → `/b_info` with (bufnum, frames, channels,
    /// sampleRate) per buffer; zeros for unallocated indices. Synchronous,
    /// answered from the mirror (= state as of the last completed command).
    fn handle_b_query(&mut self, msg: &OscMessage, from: ClientId) {
        let mut args = Vec::with_capacity(msg.args.len() * 4);
        for arg in &msg.args {
            let OscType::Int(index) = arg else {
                return self.fail(from, "/b_query", "expected int buffer indices");
            };
            let info = self.mirror_buffer(*index);
            args.push(OscType::Int(*index));
            args.push(OscType::Int(info.as_ref().map_or(0, |b| b.frames() as i32)));
            args.push(OscType::Int(
                info.as_ref().map_or(0, |b| b.channels() as i32),
            ));
            args.push(OscType::Float(
                info.as_ref().map_or(0.0, |b| b.sample_rate() as f32),
            ));
        }
        self.reply(from, "/b_info", args);
    }

    /// `/b_get bufnum index...` → `/b_set bufnum index value...`: read single
    /// samples (flat, interleaved) from the buffer mirror. Out-of-range indices
    /// (and any index into an unallocated buffer) read as `0.0`, mirroring how
    /// `Buffer::sample` and the audio-rate UGens treat them. Synchronous, like
    /// `/b_query`.
    fn handle_b_get(&mut self, msg: &OscMessage, from: ClientId) {
        let Some((OscType::Int(bufnum), indices)) = msg.args.split_first() else {
            return self.fail(from, "/b_get", "expected bufnum then int sample indices");
        };
        let buffer = self.mirror_buffer(*bufnum);
        let data = buffer.as_deref().map(|b| b.data()).unwrap_or(&[]);
        let mut args = vec![OscType::Int(*bufnum)];
        for arg in indices {
            let OscType::Int(index) = arg else {
                return self.fail(from, "/b_get", "expected int sample indices");
            };
            let value = usize::try_from(*index)
                .ok()
                .and_then(|i| data.get(i))
                .copied()
                .unwrap_or(0.0);
            args.push(OscType::Int(*index));
            args.push(OscType::Float(value));
        }
        self.reply(from, "/b_set", args);
    }

    /// `/b_getn bufnum [start count]...` → `/b_setn bufnum start count value...`:
    /// read ranges of samples (flat, interleaved) from the buffer mirror — the
    /// client-side counterpart of `/b_setn`, and how a GUI client pulls a buffer
    /// to display it. `count` is clamped to what the buffer holds from `start`,
    /// so a request past the end returns only the available samples (none for an
    /// unallocated buffer). Large buffers are read in client-chosen chunks (each
    /// reply must fit a datagram); the bulk-transfer optimization is future work.
    fn handle_b_getn(&mut self, msg: &OscMessage, from: ClientId) {
        let Some((OscType::Int(bufnum), pairs)) = msg.args.split_first() else {
            return self.fail(from, "/b_getn", "expected bufnum then (start, count) pairs");
        };
        if pairs.len() % 2 != 0 {
            return self.fail(from, "/b_getn", "expected (start, count) pairs");
        }
        let buffer = self.mirror_buffer(*bufnum);
        let data = buffer.as_deref().map(|b| b.data()).unwrap_or(&[]);
        let mut args = vec![OscType::Int(*bufnum)];
        for pair in pairs.chunks_exact(2) {
            let (OscType::Int(start), OscType::Int(count)) = (&pair[0], &pair[1]) else {
                return self.fail(from, "/b_getn", "expected int start and count");
            };
            let start = (*start).max(0) as usize;
            let count = (*count).max(0) as usize;
            let end = start.saturating_add(count).min(data.len());
            let slice = data.get(start..end).unwrap_or(&[]);
            args.push(OscType::Int(start as i32));
            args.push(OscType::Int(slice.len() as i32));
            args.extend(slice.iter().map(|s| OscType::Float(*s)));
        }
        self.reply(from, "/b_setn", args);
    }

    /// `/b_export bufnum path` → `/done /b_export bufnum`: write the buffer's raw
    /// samples (flat, interleaved, little-endian `f32`) to `path` as a **local
    /// shared resource**, so a same-machine client (the GUI host) can map and read
    /// a multi-megabyte buffer with no per-sample OSC traffic — the bulk-data path,
    /// the efficient counterpart of `/b_getn`'s chunked over-the-wire reads. The
    /// reader pairs it with the buffer's channel count (from `/b_query`) to
    /// de-interleave. Synchronous on the network thread (not the audio thread),
    /// like `/b_get`/`/b_getn`; replies `/fail` on a missing buffer or a write
    /// error.
    fn handle_b_export(&mut self, msg: &OscMessage, from: ClientId) {
        let (Some(OscType::Int(bufnum)), Some(OscType::String(path))) =
            (msg.args.first(), msg.args.get(1))
        else {
            return self.fail(from, "/b_export", "expected bufnum then a path string");
        };
        let Some(buffer) = self.mirror_buffer(*bufnum) else {
            return self.fail(from, "/b_export", "no such buffer");
        };
        let mut bytes = Vec::with_capacity(buffer.data().len() * 4);
        for &s in buffer.data() {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        match std::fs::write(path, &bytes) {
            Ok(()) => self.reply(
                from,
                "/done",
                vec![OscType::String("/b_export".into()), OscType::Int(*bufnum)],
            ),
            Err(e) => self.fail(from, "/b_export", format!("write {path}: {e}")),
        }
    }

    fn mirror_buffer(&self, index: i32) -> Option<Arc<crate::dsp::buffer::Buffer>> {
        usize::try_from(index)
            .ok()
            .and_then(|i| self.translator.buffers.get(i))
            .and_then(|b| b.as_ref().map(Arc::clone))
    }

    fn handle_notify(&mut self, msg: &OscMessage, from: ClientId) {
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
                    vec![OscType::String("/notify".into()), OscType::Int(id as i32)],
                );
            }
            Some(OscType::Int(0)) => {
                self.clients.retain(|c| *c != from);
                self.reply(from, "/done", vec![OscType::String("/notify".into())]);
            }
            _ => self.fail(from, "/notify", "expected int argument 0 or 1"),
        }
    }

    fn fail(&self, to: ClientId, cmd: &str, why: impl Into<String>) {
        self.reply(
            to,
            "/fail",
            vec![OscType::String(cmd.into()), OscType::String(why.into())],
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
                if let Err(e) = self.socket.send_to(&bytes, addr_to) {
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
            ClientId::Ws(id) => {
                // Binary-message reply on the originating connection; dropped if
                // it has since closed.
                if let Some(hub) = &self.ws {
                    hub.reply(id, &bytes);
                }
            }
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

/// The raw `SynthDefSpec` JSON of a `/d_recv` message (blob or string form),
/// for persisting it verbatim. Mirrors the argument parsing in
/// [`CmdTranslator::d_recv`].
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
/// `/clock.reply` so a client gets the anchor `(osc_time, sample)` it needs to
/// place its logical OSC time on this server's sample axis.
fn now_ntp() -> OscTime {
    let unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let ntp = unix + NTP_UNIX_OFFSET;
    let seconds = ntp.trunc();
    OscTime {
        seconds: seconds as u32,
        fractional: ((ntp - seconds) * 2f64.powi(32)) as u32,
    }
}

/// Seconds from now until the timetag fires. `None` is the OSC
/// "immediately" tag (seconds 0, fractional 1 — rosc keeps it verbatim).
fn timetag_delta_secs(t: OscTime) -> Option<f64> {
    if t.seconds == 0 && t.fractional <= 1 {
        return None;
    }
    let target = t.seconds as f64 - NTP_UNIX_OFFSET + t.fractional as f64 / 2f64.powi(32);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    Some(target - now)
}
