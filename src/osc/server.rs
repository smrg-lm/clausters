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
use crate::faust::compiler::{CompilePayload, CompileRequest, CompilerThread};
use crate::dsp::buffer::{BufferPool, empty_pool};
use crate::node::{AddAction, Group, Place};
use crate::osc::translate::{
    CmdTranslator, control_key, float_value, parse_buffer_msg,
};
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
    clients: Vec<SocketAddr>,
    recv_buf: Vec<u8>,
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
            recv_buf: vec![0; RECV_BUF_SIZE],
            #[cfg(feature = "faust")]
            faust_compiler: CompilerThread::spawn(),
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Blocks serving requests until a `/quit` arrives.
    pub fn run(&mut self) -> io::Result<()> {
        loop {
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
            // decode_packet carries the rosc 0.10 blob workaround, including
            // for blobs inside bundle elements (see `crate::osc`).
            let packet = match crate::osc::decode_packet(&self.recv_buf[..len]) {
                Ok(packet) => packet,
                Err(e) => {
                    eprintln!("malformed OSC packet from {from}: {e}");
                    continue;
                }
            };
            let flow = self.handle_packet(packet, from);
            self.collect_garbage();
            self.collect_nrt_results();
            #[cfg(feature = "faust")]
            self.collect_faust_results();
            if let Flow::Quit = flow {
                return Ok(());
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
                    self.reply(
                        result.client,
                        "/done",
                        vec![
                            OscType::String("/d_faust".into()),
                            OscType::String(result.name),
                        ],
                    );
                }
                Err(error) => self.fail(result.client, "/d_faust", error),
            }
        }
    }

    /// `/d_faust name def`: queue an async Faust compilation. The def is a
    /// JSON box graph if it starts with `{` (F2, see `faust::boxes` for the
    /// schema), raw Faust source otherwise (F1) — top-level Faust source can
    /// never start with `{`, so the sniff is unambiguous.
    #[cfg(feature = "faust")]
    fn handle_d_faust(&mut self, msg: &OscMessage, from: SocketAddr) {
        let (name, def) = match crate::osc::translate::parse_d_faust(&msg.args) {
            Ok(pair) => pair,
            Err(e) => return self.fail(from, "/d_faust", e),
        };
        let payload = if def.trim_start().starts_with('{') {
            CompilePayload::Json(def)
        } else {
            CompilePayload::Source(def)
        };
        let request = CompileRequest {
            name,
            payload,
            client: from,
        };
        if self.faust_compiler.submit(request).is_err() {
            self.fail(from, "/d_faust", "compiler thread is down");
        }
    }

    #[cfg(not(feature = "faust"))]
    fn handle_d_faust(&mut self, _msg: &OscMessage, from: SocketAddr) {
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

    fn handle_packet(&mut self, packet: OscPacket, from: SocketAddr) -> Flow {
        match packet {
            OscPacket::Message(msg) => self.handle_message(msg, from),
            OscPacket::Bundle(bundle) => self.handle_bundle(bundle, from),
        }
    }

    /// Bundles with the "immediately" timetag (or a past one — scsynth also
    /// runs late bundles right away) execute now; future timetags are
    /// converted to a sample target and shipped to the engine's scheduler,
    /// which fires them sample-accurately (M6).
    fn handle_bundle(&mut self, bundle: OscBundle, from: SocketAddr) -> Flow {
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

    fn run_bundle_now(&mut self, bundle: OscBundle, from: SocketAddr) -> Flow {
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
    fn schedule_bundle(&mut self, bundle: OscBundle, delta: f64, from: SocketAddr) {
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

    fn handle_message(&mut self, msg: OscMessage, from: SocketAddr) -> Flow {
        if self.dump_osc {
            println!("[dumpOSC] {} {:?}", msg.addr, msg.args);
        }
        match msg.addr.as_str() {
            "/status" => self.send_status(from),
            "/notify" => self.handle_notify(&msg, from),
            "/s_new" => self.handle_s_new(&msg, from),
            "/g_new" => self.handle_g_new(&msg, from),
            "/g_freeAll" => self.handle_g_free(&msg, from, "/g_freeAll"),
            "/g_deepFree" => self.handle_g_free(&msg, from, "/g_deepFree"),
            "/n_free" => self.handle_n_free(&msg, from),
            "/n_set" => self.handle_n_set(&msg, from),
            "/n_before" => self.handle_n_move(&msg, from, Place::Before),
            "/n_after" => self.handle_n_move(&msg, from, Place::After),
            "/c_set" => self.handle_c_set(&msg, from),
            "/c_get" => self.handle_c_get(&msg, from),
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

    fn send_status(&mut self, to: SocketAddr) {
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

    fn handle_d_recv(&mut self, msg: &OscMessage, from: SocketAddr) {
        match self.translator.d_recv(&msg.args) {
            Ok(()) => self.reply(from, "/done", vec![OscType::String("/d_recv".into())]),
            Err(e) => self.fail(from, "/d_recv", e),
        }
    }

    fn handle_d_free(&mut self, msg: &OscMessage, from: SocketAddr) {
        if let Err(e) = self.translator.d_free(&msg.args) {
            self.fail(from, "/d_free", e);
        }
    }

    fn handle_s_new(&mut self, msg: &OscMessage, from: SocketAddr) {
        let mut cmds = Vec::new();
        if let Err(e) = self.translator.translate(msg, &mut cmds) {
            return self.fail(from, "/s_new", e);
        }
        for cmd in cmds {
            if self.handle.send(cmd).is_err() {
                return self.fail(from, "/s_new", "command FIFO full");
            }
        }
    }

    /// `/g_new` takes (id, addAction, targetID) triples.
    fn handle_g_new(&mut self, msg: &OscMessage, from: SocketAddr) {
        if msg.args.is_empty() || !msg.args.len().is_multiple_of(3) {
            return self.fail(from, "/g_new", "expected (id, addAction, targetID) triples");
        }
        for triple in msg.args.chunks(3) {
            let [OscType::Int(id), OscType::Int(action), OscType::Int(target)] = triple else {
                return self.fail(from, "/g_new", "expected int (id, addAction, targetID)");
            };
            let Some(action) = AddAction::from_i32(*action) else {
                return self.fail(from, "/g_new", "add action must be 0-4");
            };
            if *id <= 0 {
                return self.fail(from, "/g_new", "group ID must be positive");
            }
            let cmd = Cmd::AddGroup {
                id: *id,
                target: *target,
                action,
                group: Group::new(),
            };
            if self.handle.send(cmd).is_err() {
                return self.fail(from, "/g_new", "command FIFO full");
            }
        }
    }

    fn handle_g_free(&mut self, msg: &OscMessage, from: SocketAddr, cmd_name: &str) {
        for arg in &msg.args {
            let OscType::Int(id) = arg else {
                return self.fail(from, cmd_name, "expected int group IDs");
            };
            let cmd = match cmd_name {
                "/g_freeAll" => Cmd::FreeAllInGroup { id: *id },
                _ => Cmd::DeepFreeGroup { id: *id },
            };
            if self.handle.send(cmd).is_err() {
                return self.fail(from, cmd_name, "command FIFO full");
            }
        }
    }

    /// `/n_before` / `/n_after` take (nodeID, targetID) pairs.
    fn handle_n_move(&mut self, msg: &OscMessage, from: SocketAddr, place: Place) {
        let cmd_name = match place {
            Place::Before => "/n_before",
            Place::After => "/n_after",
        };
        if msg.args.is_empty() || !msg.args.len().is_multiple_of(2) {
            return self.fail(from, cmd_name, "expected (nodeID, targetID) pairs");
        }
        for pair in msg.args.chunks(2) {
            let [OscType::Int(id), OscType::Int(target)] = pair else {
                return self.fail(from, cmd_name, "expected int (nodeID, targetID)");
            };
            let cmd = Cmd::MoveNode {
                id: *id,
                target: *target,
                place,
            };
            if self.handle.send(cmd).is_err() {
                return self.fail(from, cmd_name, "command FIFO full");
            }
        }
    }

    /// Control buses are shared atomics: set directly, no engine round-trip.
    fn handle_c_set(&mut self, msg: &OscMessage, from: SocketAddr) {
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
    fn handle_c_get(&mut self, msg: &OscMessage, from: SocketAddr) {
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
    fn handle_b_cmd(&mut self, msg: &OscMessage, from: SocketAddr, cmd: &'static str) {
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
    fn handle_b_query(&mut self, msg: &OscMessage, from: SocketAddr) {
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

    fn handle_n_free(&mut self, msg: &OscMessage, from: SocketAddr) {
        for arg in &msg.args {
            let OscType::Int(id) = arg else {
                return self.fail(from, "/n_free", "expected int node IDs");
            };
            if self.handle.send(Cmd::FreeNode { id: *id }).is_err() {
                return self.fail(from, "/n_free", "command FIFO full");
            }
        }
    }

    fn handle_n_set(&mut self, msg: &OscMessage, from: SocketAddr) {
        let Some(OscType::Int(id)) = msg.args.first() else {
            return self.fail(from, "/n_set", "expected: id, then control/value pairs");
        };
        let id = *id;
        let Some(def) = self.translator.node_defs.get(&id).cloned() else {
            return self.fail(from, "/n_set", format!("node {id} not found"));
        };
        for pair in msg.args[1..].chunks(2) {
            let (Some(index), Some(value)) = (
                control_key(&pair[0], &def),
                pair.get(1).and_then(float_value),
            ) else {
                continue;
            };
            if self
                .handle
                .send(Cmd::SetControl { id, index, value })
                .is_err()
            {
                return self.fail(from, "/n_set", "command FIFO full");
            }
        }
    }

    fn handle_notify(&mut self, msg: &OscMessage, from: SocketAddr) {
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

    fn fail(&self, to: SocketAddr, cmd: &str, why: impl Into<String>) {
        self.reply(
            to,
            "/fail",
            vec![OscType::String(cmd.into()), OscType::String(why.into())],
        );
    }

    fn reply(&self, to: SocketAddr, addr: &str, args: Vec<OscType>) {
        let packet = OscPacket::Message(OscMessage {
            addr: addr.into(),
            args,
        });
        match encoder::encode(&packet) {
            Ok(bytes) => {
                if let Err(e) = self.socket.send_to(&bytes, to) {
                    eprintln!("failed to send {addr} to {to}: {e}");
                }
            }
            Err(e) => eprintln!("failed to encode {addr}: {e}"),
        }
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

