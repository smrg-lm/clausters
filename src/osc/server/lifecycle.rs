//! Bringing the server up, running it, and the housekeeping the loop does
//! between packets.
//!
//! [`OscServer::run`] is the network thread: it drains every carrier, pumps the
//! subscriptions, collects what the async pipelines finished, and drops the
//! garbage the audio thread handed back. [`OscServer::step`] is the same turn
//! for a host that drives the engine itself (headless/wasm), where there is no
//! blocking recv to wait on.

use super::*;

/// The def's name as the store spells it: the file stem, which is what
/// `DefStore` writes a def under.
fn def_name(path: &std::path::Path) -> std::borrow::Cow<'_, str> {
    path.file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_else(|| path.to_string_lossy())
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
        // The async workers end that wait the moment a result lands, so the
        // interval above is housekeeping and not the latency of a reply.
        let mut wake_target = socket.local_addr()?;
        if wake_target.ip().is_unspecified() {
            wake_target.set_ip(match wake_target {
                SocketAddr::V4(_) => std::net::Ipv4Addr::LOCALHOST.into(),
                SocketAddr::V6(_) => std::net::Ipv6Addr::LOCALHOST.into(),
            });
        }
        let waker = crate::osc::wake::Waker::to(wake_target).ok();
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
            nrt: NrtRunner::spawn(waker.clone()),
            clients: Vec::new(),
            streams: Vec::new(),
            tap_streams: Vec::new(),
            buffer_streams: Vec::new(),
            tap_rings: Vec::new(),
            tap_refs: Vec::new(),
            tap_buf: Vec::new(),
            clock: TimeSource::Wall {
                epoch: Instant::now(),
            },
            ipc: None,
            segment: None,
            shm_path: None,
            owns_samples: false,
            shared_buffers: Vec::new(),
            tcp: None,
            #[cfg(not(target_arch = "wasm32"))]
            ws: None,
            #[cfg(feature = "midi")]
            midi: None,
            store: None,
            prune_dead_defs: false,
            recv_buf: vec![0; RECV_BUF_SIZE],
            #[cfg(feature = "faust")]
            faust_compiler: CompilerThread::spawn(waker),
            nrt_submitted: 0,
            nrt_in_flight: Default::default(),
            nrt_drained: 0,
            faust_submitted: 0,
            faust_drained: 0,
            pending_syncs: Vec::new(),
            transport: Transport::default(),
            post_errors: true,
            max_frame: crate::osc::DEFAULT_MAX_FRAME,
            max_clients: crate::osc::DEFAULT_MAX_CLIENTS,
            client_slots: None,
            offline: None,
        })
    }

    /// A server with **no socket front** — the pulled mode. Commands and
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
            buffer_streams: Vec::new(),
            tap_rings: Vec::new(),
            tap_refs: Vec::new(),
            tap_buf: Vec::new(),
            clock: TimeSource::Sample { unix_epoch },
            ipc: None,
            segment: None,
            shm_path: None,
            owns_samples: false,
            shared_buffers: Vec::new(),
            tcp: None,
            #[cfg(not(target_arch = "wasm32"))]
            ws: None,
            #[cfg(feature = "midi")]
            midi: None,
            store: None,
            prune_dead_defs: false,
            recv_buf: vec![0; RECV_BUF_SIZE],
            #[cfg(feature = "faust")]
            faust_compiler: CompilerThread::spawn(None),
            nrt_submitted: 0,
            nrt_in_flight: Default::default(),
            nrt_drained: 0,
            faust_submitted: 0,
            faust_drained: 0,
            pending_syncs: Vec::new(),
            transport: Transport::default(),
            post_errors: true,
            max_frame: crate::osc::DEFAULT_MAX_FRAME,
            max_clients: crate::osc::DEFAULT_MAX_CLIENTS,
            client_slots: None,
            offline: None,
        }
    }

    /// Drops a persisted def that will not load instead of warning about it
    /// (`--prune-defs`). Set it **before** [`Self::attach_store`], which is
    /// where the reload happens.
    pub fn prune_dead_defs(&mut self, on: bool) {
        self.prune_dead_defs = on;
    }

    /// Enables on-disk persistence and reloads whatever defs the store
    /// already holds. SynthDefs are recompiled inline (cheap); Faust defs are
    /// queued on the compiler thread, restoring from the bitcode cache when
    /// possible, so the socket starts serving immediately and the library
    /// loads incrementally.
    pub fn attach_store(&mut self, store: DefStore) {
        let mut dead: Vec<std::path::PathBuf> = Vec::new();
        #[cfg(feature = "synth")]
        for (path, spec) in store.load_synthdef_specs() {
            if let Err(e) = self.translator.d_recv(&[OscType::Blob(spec)]) {
                warn!(
                    "persisted SynthDef '{}' failed to load: {e}",
                    def_name(&path)
                );
                dead.push(path);
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
        // missing member only fails later at /graph_new.
        for (path, spec) in store.load_graphdef_specs() {
            if let Err(e) = self.translator.d_graph(&[OscType::Blob(spec)]) {
                warn!(
                    "persisted GraphDef '{}' failed to load: {e}",
                    def_name(&path)
                );
                dead.push(path);
            }
        }
        self.retire_dead_defs(&dead, store.defs_dir());
        // Boot order: defs -> graphdefs -> bindings -> boot preset, so a
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

    /// What a reload does with the defs that did not load: name them, or —
    /// under `--prune-defs` — drop them.
    ///
    /// Failing to load is **not** by itself a reason to delete: a def whose
    /// family this build lacks fails too, and a `--no-default-features` boot
    /// would eat the library. So the default is to say which def and where the
    /// library is, once, with the one command that clears it — the warnings
    /// were repeating every boot forever with no way to tell what they were
    /// about (a `PlayBuf` that grew from four inputs to seven left seven of
    /// them on the author's machine).
    fn retire_dead_defs(&self, dead: &[std::path::PathBuf], defs_dir: &std::path::Path) {
        if dead.is_empty() {
            return;
        }
        if !self.prune_dead_defs {
            warn!(
                "{} persisted def(s) did not load, and will warn again at every boot; they are in \
                 {} — drop them with `clausters --prune-defs`",
                dead.len(),
                defs_dir.display()
            );
            return;
        }
        for path in dead {
            match std::fs::remove_file(path) {
                Ok(()) => warn!("pruned the persisted def '{}'", def_name(path)),
                Err(e) => warn!("cannot prune {}: {e}", path.display()),
            }
        }
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
            self.pump_buffer_streams();
            let socket = self.socket.as_ref().expect("run() checked the socket");
            let (len, from) = match socket.recv_from(&mut self.recv_buf) {
                Ok(ok) => ok,
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    self.collect_async();
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
                // A zero-length datagram is a wake: a reader queued a TCP frame
                // or a disconnect, or a worker thread finished a job
                // (`crate::osc::wake`). Collect before looping back, since a
                // finished result is reported from here and nowhere else.
                self.collect_async();
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
            self.collect_async();
            if let Flow::Quit = flow {
                return Ok(());
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
        self.pump_buffer_streams();
        self.collect_async();
        false
    }

    /// One housekeeping pass: the garbage the audio thread handed back, and
    /// whatever the async workers finished. Every path out of the recv runs
    /// it — a packet, an idle tick, and a wake datagram alike — so a result is
    /// reported as soon as the loop learns of it.
    pub(in crate::osc::server) fn collect_async(&mut self) {
        self.collect_garbage();
        self.collect_nrt_results();
        #[cfg(feature = "faust")]
        self.collect_faust_results();
    }

    /// Drops what the audio thread discarded, keeps the def mirror in sync
    /// and forwards node lifecycle events to `/server_notify` clients.
    pub(in crate::osc::server) fn collect_garbage(&mut self) {
        while let Some(g) = self.handle.pop_garbage() {
            match g {
                Garbage::FreedSynth { id, .. } => {
                    self.translator.forget_node(id);
                }
                Garbage::FreedGroup { id, .. } => {
                    // A governed group that has been freed cannot govern
                    // anything: unbind rather than leave the transport pointing
                    // at a node that no longer exists.
                    if self.transport.group == Some(id) {
                        self.transport.group = None;
                        self.handle.send(Cmd::TransportGroup { id: -1 }).ok();
                        self.broadcast_transport();
                    }
                }
                Garbage::FreedBuffer(_) => {}
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
        // Side-effect replies: `SendTrig`/`SendReply` reply to `/server_notify`
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

    /// Retunes the socket read timeout — the run loop's idle tick — to the
    /// fastest subscribed stream period, so streams keep their cadence without
    /// traffic. The 2 ms IPC poll (`attach_ipc`) is faster than any allowed
    /// period and wins unconditionally; without streams the tick falls back to
    /// the GC interval.
    pub(in crate::osc::server) fn retune_timeout(&self) {
        if self.ipc.is_some() {
            return;
        }
        let timeout = self
            .streams
            .iter()
            .map(|s| s.period)
            .chain(self.tap_streams.iter().map(|s| s.period))
            .chain(self.buffer_streams.iter().map(|s| s.period))
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
        self.buffer_streams.retain(|s| !gone.contains(&s.client));
        self.clients.retain(|c| !gone.contains(c));
        self.retune_timeout();
    }
}
