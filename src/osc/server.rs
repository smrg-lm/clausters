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

use std::collections::HashMap;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use rosc::{OscBundle, OscMessage, OscPacket, OscType, decoder, encoder};

#[cfg(feature = "faust")]
use crate::faust::compiler::{CompilePayload, CompileRequest, CompilerThread};
#[cfg(feature = "faust")]
use crate::faust::synth::{FaustDef, FaustSynth};
use crate::dsp::buffer::{BufferPool, NUM_BUFFERS, empty_pool};
use crate::node::{AddAction, Group, Place, SynthNode};
use crate::server::engine::{Cmd, EngineHandle, Garbage, NodeEventKind};
use crate::server::nrt::{NrtAction, NrtJob, NrtRequest, NrtThread};
use crate::synthdef::instance::UGenSynth;
use crate::synthdef::{SynthDef, SynthDefSpec, compile, default_spec};

/// Default scsynth port.
pub const DEFAULT_PORT: u16 = 57110;

/// Largest UDP datagram we accept.
const RECV_BUF_SIZE: usize = 65536;

/// How long `recv_from` blocks before we take a garbage-collection pass.
const GC_INTERVAL: Duration = Duration::from_millis(100);

/// Auto-assigned node IDs (`/s_new` with ID -1) start above this.
const AUTO_NODE_ID_BASE: i32 = 2_000_000;

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

/// What a live node was built from, mirrored per node ID so `/n_set` can
/// resolve control names on the network thread.
#[derive(Clone)]
enum NodeDef {
    UGen(Arc<SynthDef>),
    #[cfg(feature = "faust")]
    Faust(Arc<FaustDef>),
}

impl NodeDef {
    fn control_index(&self, name: &str) -> Option<u32> {
        match self {
            NodeDef::UGen(def) => def.control_index(name),
            #[cfg(feature = "faust")]
            NodeDef::Faust(def) => def.control_index(name),
        }
    }
}

pub struct OscServer {
    socket: UdpSocket,
    info: ServerInfo,
    handle: EngineHandle,
    /// Loaded SynthDefs. Only the network thread needs them: instances are
    /// built here, the audio thread just runs them.
    defs: HashMap<String, Arc<SynthDef>>,
    /// Mirror of which def each live node was built from, for resolving
    /// `/n_set` control names. Maintained from s_new and collected garbage.
    node_defs: HashMap<i32, NodeDef>,
    dump_osc: bool,
    /// Network-side mirror of the engine's buffer pool, updated when NRT
    /// results are installed. Serves `/b_query` and gives `/b_read`,
    /// `/b_write` and `/b_zero` the current contents/shape.
    buffers: BufferPool,
    nrt: NrtThread,
    /// Clients registered via `/notify 1`; the client ID is index + 1.
    clients: Vec<SocketAddr>,
    recv_buf: Vec<u8>,
    next_auto_id: i32,
    /// Compiled Faust defs by name, refcounted (every instance holds a
    /// clone). The compiler thread is owned here and dies with the server.
    #[cfg(feature = "faust")]
    faust_defs: HashMap<String, Arc<FaustDef>>,
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
        let mut defs = HashMap::new();
        let default = compile(default_spec()).expect("built-in default def must compile");
        defs.insert(default.name.clone(), Arc::new(default));
        Ok(Self {
            socket,
            info,
            handle,
            defs,
            node_defs: HashMap::new(),
            dump_osc: false,
            buffers: empty_pool(),
            nrt: NrtThread::spawn(),
            clients: Vec::new(),
            recv_buf: vec![0; RECV_BUF_SIZE],
            next_auto_id: AUTO_NODE_ID_BASE,
            #[cfg(feature = "faust")]
            faust_defs: HashMap::new(),
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
            // rosc 0.10's decoder over-reads the padding of blobs whose
            // length is a multiple of 4, returning Eof on valid packets.
            // Four appended zero bytes are harmless for well-formed packets
            // (left as unparsed remainder) and let those blobs decode.
            let end = (len + 4).min(self.recv_buf.len());
            self.recv_buf[len..end].fill(0);
            let packet = match decoder::decode_udp(&self.recv_buf[..end]) {
                Ok((_, packet)) => packet,
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
                    self.faust_defs.insert(result.name.clone(), Arc::new(def));
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
        let (name, def) = match msg.args.as_slice() {
            [OscType::String(name), OscType::String(src), ..] => (name.clone(), src.clone()),
            [OscType::String(name), OscType::Blob(src), ..] => (
                name.clone(),
                match String::from_utf8(src.clone()) {
                    Ok(s) => s,
                    Err(_) => return self.fail(from, "/d_faust", "def blob is not UTF-8"),
                },
            ),
            _ => return self.fail(from, "/d_faust", "expected: name, JSON or Faust source"),
        };
        if name.is_empty() {
            return self.fail(from, "/d_faust", "empty def name");
        }
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
                    self.node_defs.remove(&id);
                }
                Garbage::FreedGroup { .. } | Garbage::FreedBuffer(_) => {}
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

    /// M3 executes bundle contents immediately; timetag scheduling is M6.
    fn handle_bundle(&mut self, bundle: OscBundle, from: SocketAddr) -> Flow {
        for packet in bundle.content {
            if let Flow::Quit = self.handle_packet(packet, from) {
                return Flow::Quit;
            }
        }
        Flow::Continue
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
            "/b_alloc" => self.handle_b_alloc(&msg, from),
            "/b_allocRead" => self.handle_b_alloc_read(&msg, from),
            "/b_read" => self.handle_b_read(&msg, from),
            "/b_write" => self.handle_b_write(&msg, from),
            "/b_zero" => self.handle_b_zero(&msg, from),
            "/b_free" => self.handle_b_free(&msg, from),
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
        #[allow(unused_mut)]
        let mut num_defs = self.defs.len();
        #[cfg(feature = "faust")]
        {
            num_defs += self.faust_defs.len();
        }
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
        let bytes: &[u8] = match msg.args.first() {
            Some(OscType::Blob(b)) => b,
            Some(OscType::String(s)) => s.as_bytes(),
            _ => return self.fail(from, "/d_recv", "expected a JSON blob or string"),
        };
        let spec: SynthDefSpec = match serde_json::from_slice(bytes) {
            Ok(spec) => spec,
            Err(e) => return self.fail(from, "/d_recv", format!("invalid JSON: {e}")),
        };
        match compile(spec) {
            Ok(def) => {
                self.defs.insert(def.name.clone(), Arc::new(def));
                self.reply(from, "/done", vec![OscType::String("/d_recv".into())]);
            }
            Err(e) => self.fail(from, "/d_recv", e),
        }
    }

    fn handle_d_free(&mut self, msg: &OscMessage, from: SocketAddr) {
        for arg in &msg.args {
            let OscType::String(name) = arg else {
                return self.fail(from, "/d_free", "expected synthdef names");
            };
            // Live synths keep their Arc<SynthDef>: scsynth semantics. Same
            // for Faust factories (instances refcount them).
            self.defs.remove(name);
            #[cfg(feature = "faust")]
            self.faust_defs.remove(name);
        }
    }

    fn handle_s_new(&mut self, msg: &OscMessage, from: SocketAddr) {
        let (def_name, id, action, target) = match msg.args.as_slice() {
            [
                OscType::String(def),
                OscType::Int(id),
                OscType::Int(action),
                OscType::Int(target),
                ..,
            ] => (def.clone(), *id, *action, *target),
            _ => return self.fail(from, "/s_new", "expected: name, id, addAction, targetID"),
        };
        let (mut synth, def) = match self.make_synth(&def_name) {
            Ok(pair) => pair,
            Err(e) => return self.fail(from, "/s_new", e),
        };
        let Some(action) = AddAction::from_i32(action) else {
            return self.fail(from, "/s_new", "add action must be 0-4");
        };
        let id = if id == -1 {
            self.next_auto_id += 1;
            self.next_auto_id
        } else if id > 0 {
            id
        } else {
            return self.fail(from, "/s_new", "node ID must be positive or -1");
        };

        for pair in msg.args[4..].chunks(2) {
            let (Some(index), Some(value)) = (
                control_key(&pair[0], &def),
                pair.get(1).and_then(float_value),
            ) else {
                continue; // unknown controls are ignored, like scsynth
            };
            synth.set_control(index, value);
        }

        let cmd = Cmd::AddSynth {
            id,
            target,
            action,
            synth,
        };
        if self.handle.send(cmd).is_ok() {
            self.node_defs.insert(id, def);
        } else {
            self.fail(from, "/s_new", "command FIFO full");
        }
    }

    /// Builds a synth instance from either def table. Faust instantiation
    /// (`createCDSPInstance` + `init`) allocates — fine, this is the network
    /// thread; the boxed instance reaches the audio thread fully built.
    fn make_synth(&self, name: &str) -> Result<(Box<dyn SynthNode>, NodeDef), String> {
        if let Some(def) = self.defs.get(name) {
            let synth = Box::new(UGenSynth::new(Arc::clone(def)));
            return Ok((synth, NodeDef::UGen(Arc::clone(def))));
        }
        #[cfg(feature = "faust")]
        if let Some(def) = self.faust_defs.get(name) {
            let synth = FaustSynth::new(Arc::clone(def), self.handle.sample_rate)?;
            return Ok((Box::new(synth), NodeDef::Faust(Arc::clone(def))));
        }
        Err(format!("SynthDef not found: {name}"))
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

    /// Validates a buffer index and queues an NRT job for it.
    fn submit_nrt(&mut self, cmd: &'static str, index: i32, from: SocketAddr, job: NrtJob) {
        if index < 0 || index as usize >= NUM_BUFFERS {
            return self.fail(from, cmd, format!("buffer index out of range: {index}"));
        }
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

    /// `/b_alloc bufnum frames [channels=1]`.
    fn handle_b_alloc(&mut self, msg: &OscMessage, from: SocketAddr) {
        let (index, frames) = match msg.args.as_slice() {
            [OscType::Int(index), OscType::Int(frames), ..] => (*index, *frames),
            _ => return self.fail(from, "/b_alloc", "expected: bufnum, frames [, channels]"),
        };
        let channels = int_arg(&msg.args, 2).unwrap_or(1);
        if frames <= 0 || channels <= 0 {
            return self.fail(from, "/b_alloc", "frames and channels must be positive");
        }
        let job = NrtJob::Alloc {
            frames: frames as usize,
            channels: channels as usize,
            sample_rate: self.info.nominal_sample_rate,
        };
        self.submit_nrt("/b_alloc", index, from, job);
    }

    /// `/b_allocRead bufnum path [fileStart=0] [numFrames=0 (all)]`.
    fn handle_b_alloc_read(&mut self, msg: &OscMessage, from: SocketAddr) {
        let (index, path) = match msg.args.as_slice() {
            [OscType::Int(index), OscType::String(path), ..] => (*index, path.clone()),
            _ => {
                return self.fail(
                    from,
                    "/b_allocRead",
                    "expected: bufnum, path [, fileStart, numFrames]",
                );
            }
        };
        let job = NrtJob::AllocRead {
            path,
            file_start: int_arg(&msg.args, 2).unwrap_or(0).max(0) as usize,
            num_frames: int_arg(&msg.args, 3).unwrap_or(0) as i64,
        };
        self.submit_nrt("/b_allocRead", index, from, job);
    }

    /// `/b_read bufnum path [fileStart=0] [numFrames=-1 (all)] [bufStart=0]
    /// [leaveOpen]` — leaveOpen is accepted and ignored (no streaming yet).
    /// The buffer must already exist; its shape is kept.
    fn handle_b_read(&mut self, msg: &OscMessage, from: SocketAddr) {
        let (index, path) = match msg.args.as_slice() {
            [OscType::Int(index), OscType::String(path), ..] => (*index, path.clone()),
            _ => {
                return self.fail(
                    from,
                    "/b_read",
                    "expected: bufnum, path [, fileStart, numFrames, bufStart]",
                );
            }
        };
        let Some(current) = self.mirror_buffer(index) else {
            return self.fail(from, "/b_read", format!("no buffer allocated at {index}"));
        };
        let job = NrtJob::Read {
            path,
            file_start: int_arg(&msg.args, 2).unwrap_or(0).max(0) as usize,
            num_frames: int_arg(&msg.args, 3).unwrap_or(-1) as i64,
            buf_start: int_arg(&msg.args, 4).unwrap_or(0).max(0) as usize,
            current,
        };
        self.submit_nrt("/b_read", index, from, job);
    }

    /// `/b_write bufnum path [headerFormat="wav"] [sampleFormat="int16"]
    /// [numFrames=-1 (all)] [startFrame=0]` — WAV only in v1.
    fn handle_b_write(&mut self, msg: &OscMessage, from: SocketAddr) {
        let (index, path) = match msg.args.as_slice() {
            [OscType::Int(index), OscType::String(path), ..] => (*index, path.clone()),
            _ => {
                return self.fail(
                    from,
                    "/b_write",
                    "expected: bufnum, path [, headerFormat, sampleFormat, numFrames, startFrame]",
                );
            }
        };
        let header = string_arg(&msg.args, 2).unwrap_or("wav");
        if !header.eq_ignore_ascii_case("wav") && !header.eq_ignore_ascii_case("wave") {
            return self.fail(from, "/b_write", format!("unsupported header format {header:?}"));
        }
        let sample_format = string_arg(&msg.args, 3).unwrap_or("int16").to_string();
        let Some(buffer) = self.mirror_buffer(index) else {
            return self.fail(from, "/b_write", format!("no buffer allocated at {index}"));
        };
        let job = NrtJob::Write {
            path,
            sample_format,
            num_frames: int_arg(&msg.args, 4).unwrap_or(-1) as i64,
            buf_start: int_arg(&msg.args, 5).unwrap_or(0).max(0) as usize,
            buffer,
        };
        self.submit_nrt("/b_write", index, from, job);
    }

    /// `/b_zero bufnum`: replaces the buffer with a zeroed one of the same
    /// shape (buffers are immutable; see `dsp::buffer`).
    fn handle_b_zero(&mut self, msg: &OscMessage, from: SocketAddr) {
        let Some(OscType::Int(index)) = msg.args.first() else {
            return self.fail(from, "/b_zero", "expected a buffer index");
        };
        let index = *index;
        let Some(current) = self.mirror_buffer(index) else {
            return self.fail(from, "/b_zero", format!("no buffer allocated at {index}"));
        };
        let job = NrtJob::Alloc {
            frames: current.frames(),
            channels: current.channels(),
            sample_rate: current.sample_rate(),
        };
        self.submit_nrt("/b_zero", index, from, job);
    }

    /// `/b_free bufnum`: routed through the NRT queue so it cannot overtake
    /// a pending alloc/read on the same index.
    fn handle_b_free(&mut self, msg: &OscMessage, from: SocketAddr) {
        let Some(OscType::Int(index)) = msg.args.first() else {
            return self.fail(from, "/b_free", "expected a buffer index");
        };
        self.submit_nrt("/b_free", *index, from, NrtJob::Free);
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
        let Some(def) = self.node_defs.get(&id).cloned() else {
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

/// Control reference: by name (resolved against the def) or by index.
fn control_key(arg: &OscType, def: &NodeDef) -> Option<u32> {
    match arg {
        OscType::String(name) => def.control_index(name),
        OscType::Int(i) if *i >= 0 => Some(*i as u32),
        _ => None,
    }
}

/// Optional trailing int argument (scsynth buffer commands have several).
fn int_arg(args: &[OscType], n: usize) -> Option<i32> {
    match args.get(n) {
        Some(OscType::Int(i)) => Some(*i),
        _ => None,
    }
}

fn string_arg(args: &[OscType], n: usize) -> Option<&str> {
    match args.get(n) {
        Some(OscType::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

fn float_value(arg: &OscType) -> Option<f32> {
    match arg {
        OscType::Float(f) => Some(*f),
        OscType::Int(i) => Some(*i as f32),
        OscType::Double(d) => Some(*d as f32),
        _ => None,
    }
}
