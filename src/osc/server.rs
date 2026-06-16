//! UDP OSC server implementing the M5 subset of the scsynth protocol:
//! `/status`, `/quit`, `/notify`, `/dumpOSC`, `/s_new` (add actions 0-4),
//! `/n_free`, `/n_set`, `/n_before`, `/n_after`, `/g_new`, `/g_freeAll`,
//! `/g_deepFree`, `/c_set`, `/c_get`, `/d_recv`, `/d_free`; the buffer
//! commands `/b_alloc`, `/b_allocRead`, `/b_read`, `/b_write`, `/b_zero`,
//! `/b_free` (all async via the NRT thread, replying `/done cmd bufnum`)
//! and `/b_query` (synchronous `/b_info`); `/n_go` and
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

#[cfg(feature = "faust")]
use crate::faust::compiler::{CacheJob, CompilePayload, CompileRequest, CompilerThread};
use crate::dsp::buffer::{BufferPool, empty_pool};
use crate::osc::ClientId;
use crate::osc::translate::{CmdTranslator, float_value, int_arg, parse_buffer_msg};
use crate::server::defstore::DefStore;
use crate::server::engine::{Cmd, EngineHandle, Garbage, NodeEventKind};
use crate::server::nrt::{NrtAction, NrtRequest, NrtThread};

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
    translator: CmdTranslator,
    dump_osc: bool,
    /// Network-side mirror of the engine's buffer pool, updated when NRT
    /// results are installed. Serves `/b_query` and gives `/b_read`,
    /// `/b_write` and `/b_zero` the current contents/shape.
    buffers: BufferPool,
    nrt: NrtThread,
    /// Clients registered via `/notify 1`; the client ID is index + 1.
    clients: Vec<ClientId>,
    recv_buf: Vec<u8>,
    /// M14: the shared-memory / in-process ring endpoint, when attached.
    ipc: Option<crate::server::ipc::IpcPeer>,
    /// On-disk def persistence, when a data directory is configured. Defs
    /// loaded from it on startup; `/d_recv`/`/d_faust` write to it,
    /// `/d_free` deletes from it.
    store: Option<DefStore>,
    /// The compiler thread is owned here and dies with the server.
    #[cfg(feature = "faust")]
    faust_compiler: CompilerThread,
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
        let translator = CmdTranslator::new(handle.sample_rate);
        Ok(Self {
            socket,
            info,
            handle,
            translator,
            dump_osc: false,
            buffers: empty_pool(),
            nrt: NrtThread::spawn(),
            clients: Vec::new(),
            ipc: None,
            store: None,
            recv_buf: vec![0; RECV_BUF_SIZE],
            #[cfg(feature = "faust")]
            faust_compiler: CompilerThread::spawn(),
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
                eprintln!("persisted SynthDef failed to load: {e}");
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
                eprintln!("compiler thread down: cannot reload persisted Faust defs");
                break;
            }
        }
        self.store = Some(store);
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
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
            // The single decode entry point for every transport (`crate::osc`).
            let packet = match crate::osc::decode_packet(&self.recv_buf[..len]) {
                Ok(packet) => packet,
                Err(e) => {
                    eprintln!("malformed OSC packet from {from}: {e}");
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
                    eprintln!("malformed OSC packet from ring client: {e}");
                    continue;
                }
            };
            if let Flow::Quit = self.handle_packet(packet, ClientId::Ring) {
                return Flow::Quit;
            }
        }
    }

    /// Drains finished compilations: stores factories and sends the async
    /// `/done`/`/fail` replies. Called from the same places as
    /// `collect_garbage` (after each packet and on the GC tick).
    #[cfg(feature = "faust")]
    fn collect_faust_results(&mut self) {
        while let Some(result) = self.faust_compiler.try_result() {
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
                    None => eprintln!("persisted Faust def '{}' failed: {error}", result.name),
                },
            }
        }
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
        }
    }

    #[cfg(not(feature = "faust"))]
    fn handle_d_faust(&mut self, _msg: &OscMessage, from: ClientId) {
        self.fail(from, "/d_faust", "server built without faust support");
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
                        eprintln!("engine rejected a timed bundle (schedule queue full)");
                    }
                }
                Garbage::RejectedSynth { id, .. } | Garbage::RejectedGroup { id, .. } => {
                    // Don't touch the mirror: on a duplicate-ID rejection the
                    // original node is still alive under this ID.
                    eprintln!("engine rejected node {id} (duplicate ID, bad target or full table)");
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
                eprintln!("late bundle ({:.3}s): executing immediately", -delta);
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
                    if self.dump_osc {
                        println!("[dumpOSC] {} {:?} (in {delta:.3}s)", msg.addr, msg.args);
                    }
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
        if !cmds.is_empty()
            && self.handle.send(Cmd::Schedule { time, cmds }).is_err()
        {
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
        if self.dump_osc {
            println!("[dumpOSC] {} {:?}", msg.addr, msg.args);
        }
        match msg.addr.as_str() {
            "/status" => self.send_status(from),
            "/notify" => self.handle_notify(&msg, from),
            // The translator covers the whole schedulable subset (and keeps
            // the M12 tree mirror in sync), so the immediate forms share one
            // path: translate, then ship every command.
            "/s_new" | "/g_new" | "/g_freeAll" | "/g_deepFree" | "/n_free" | "/n_set"
            | "/n_map" | "/n_mapa" | "/n_before" | "/n_after" | "/g_sortMode"
            | "/g_parallel" => self.handle_via_translate(&msg, from),
            "/g_queryTree" => self.handle_g_query_tree(&msg, from),
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
            "/b_free" => self.handle_b_cmd(&msg, from, "/b_free"),
            "/b_query" => self.handle_b_query(&msg, from),
            "/d_recv" => self.handle_d_recv(&msg, from),
            "/d_faust" => self.handle_d_faust(&msg, from),
            "/d_free" => self.handle_d_free(&msg, from),
            "/dumpOSC" => {
                self.dump_osc = matches!(msg.args.first(), Some(OscType::Int(n)) if *n != 0);
            }
            "/quit" => {
                self.reply(from, "/done", vec![OscType::String("/quit".into())]);
                return Flow::Quit;
            }
            other => self.fail(from, other, "unknown command"),
        }
        Flow::Continue
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

    /// M8: the sample-clock query. Replies `/clock.reply` with the engine's
    /// sample counter (int64 `h`) and the actual sample rate (double `d`).
    /// Clients pair the reply with their local monotonic clock to model
    /// `sample(t_local) = a + b·t` and then schedule with [`/sched`]
    /// (`Self::handle_sched`) directly in samples — see
    /// `docs/sample-clock.md`. The counter counts *processed* samples: it
    /// runs a device buffer ahead of the speakers and pauses on xruns.
    fn handle_clock(&mut self, from: ClientId) {
        let args = vec![
            OscType::Long(self.handle.current_samples() as i64),
            OscType::Double(self.info.actual_sample_rate),
        ];
        self.reply(from, "/clock.reply", args);
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
                if self.dump_osc {
                    println!("[dumpOSC] {} {:?} (at sample {target})", msg.addr, msg.args);
                }
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
                    eprintln!("could not persist SynthDef '{name}': {e}");
                }
                self.reply(from, "/done", vec![OscType::String("/d_recv".into())]);
            }
            Err(e) => self.fail(from, "/d_recv", e),
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
                    #[cfg(feature = "faust")]
                    crate::faust::cache::remove(store.faustdefs_dir(), name);
                }
            }
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
                    self.buffers[index] = Some(Arc::clone(&buffer));
                    Some(Some(buffer))
                }
                NrtAction::Clear => {
                    self.buffers[index] = None;
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
    }

    /// Any of the async `/b_*` commands: parsing is shared with the NRT
    /// renderer; the job runs on the NRT thread. `/b_free` also travels
    /// through the queue so it cannot overtake a pending alloc/read on the
    /// same index.
    fn handle_b_cmd(&mut self, msg: &OscMessage, from: ClientId, cmd: &'static str) {
        let (index, job) =
            match parse_buffer_msg(cmd, &msg.args, &self.buffers, self.info.nominal_sample_rate) {
                Ok(parsed) => parsed,
                Err(e) => return self.fail(from, cmd, e),
            };
        let request = NrtRequest {
            cmd,
            index,
            client: from,
            job,
        };
        if self.nrt.submit(request).is_err() {
            self.fail(from, cmd, "NRT thread is down");
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
            args.push(OscType::Int(info.as_ref().map_or(0, |b| b.channels() as i32)));
            args.push(OscType::Float(
                info.as_ref().map_or(0.0, |b| b.sample_rate() as f32),
            ));
        }
        self.reply(from, "/b_info", args);
    }

    fn mirror_buffer(&self, index: i32) -> Option<Arc<crate::dsp::buffer::Buffer>> {
        usize::try_from(index)
            .ok()
            .and_then(|i| self.buffers.get(i))
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
            Err(e) => return eprintln!("failed to encode {addr}: {e}"),
        };
        match to {
            ClientId::Udp(addr_to) => {
                if let Err(e) = self.socket.send_to(&bytes, addr_to) {
                    eprintln!("failed to send {addr} to {addr_to}: {e}");
                }
            }
            ClientId::Ring => {
                // Backpressure, not loss: a full reply ring means the client
                // stopped draining; dropping the reply is all we can do
                // without blocking the server.
                if let Some(ipc) = &self.ipc
                    && !ipc.push(&bytes)
                {
                    eprintln!("reply ring full: dropping {addr}");
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

