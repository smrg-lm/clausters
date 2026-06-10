//! UDP OSC server implementing the M3 subset of the scsynth protocol:
//! `/status`, `/quit`, `/notify`, `/dumpOSC`, `/s_new`, `/n_free`, `/n_set`,
//! `/d_recv`, `/d_free`.
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

use crate::node::{AddAction, SynthNode};
use crate::server::engine::{Cmd, EngineHandle, Garbage};
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

pub struct OscServer {
    socket: UdpSocket,
    info: ServerInfo,
    handle: EngineHandle,
    /// Loaded SynthDefs. Only the network thread needs them: instances are
    /// built here, the audio thread just runs them.
    defs: HashMap<String, Arc<SynthDef>>,
    /// Mirror of which def each live node was built from, for resolving
    /// `/n_set` control names. Maintained from s_new and collected garbage.
    node_defs: HashMap<i32, Arc<SynthDef>>,
    dump_osc: bool,
    /// Clients registered via `/notify 1`; the client ID is index + 1.
    clients: Vec<SocketAddr>,
    recv_buf: Vec<u8>,
    next_auto_id: i32,
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
            clients: Vec::new(),
            recv_buf: vec![0; RECV_BUF_SIZE],
            next_auto_id: AUTO_NODE_ID_BASE,
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
            if let Flow::Quit = flow {
                return Ok(());
            }
        }
    }

    /// Drops what the audio thread discarded and keeps the def mirror in sync.
    fn collect_garbage(&mut self) {
        while let Some(g) = self.handle.pop_garbage() {
            match g {
                Garbage::Freed { id, .. } => {
                    self.node_defs.remove(&id);
                }
                Garbage::Rejected { id, .. } => {
                    // Don't touch the mirror: on a duplicate-ID rejection the
                    // original node is still alive under this ID.
                    eprintln!("engine rejected node {id} (duplicate ID or full node table)");
                }
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
            "/n_free" => self.handle_n_free(&msg, from),
            "/n_set" => self.handle_n_set(&msg, from),
            "/d_recv" => self.handle_d_recv(&msg, from),
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
        let args = vec![
            OscType::Int(1),
            OscType::Int(counters.ugens.load(Ordering::Relaxed) as i32),
            OscType::Int(counters.synths.load(Ordering::Relaxed) as i32),
            OscType::Int(1), // groups: only the root until M4
            OscType::Int(self.defs.len() as i32),
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
            // Live synths keep their Arc<SynthDef>: scsynth semantics.
            self.defs.remove(name);
        }
    }

    fn handle_s_new(&mut self, msg: &OscMessage, from: SocketAddr) {
        let (def_name, id, action) = match msg.args.as_slice() {
            [
                OscType::String(def),
                OscType::Int(id),
                OscType::Int(action),
                OscType::Int(_target),
                ..,
            ] => (def.clone(), *id, *action),
            _ => return self.fail(from, "/s_new", "expected: name, id, addAction, targetID"),
        };
        let Some(def) = self.defs.get(&def_name).cloned() else {
            return self.fail(from, "/s_new", format!("SynthDef not found: {def_name}"));
        };
        let action = match action {
            0 => AddAction::Head,
            1 => AddAction::Tail,
            _ => return self.fail(from, "/s_new", "add actions 2-4 arrive in M4"),
        };
        let id = if id == -1 {
            self.next_auto_id += 1;
            self.next_auto_id
        } else if id > 0 {
            id
        } else {
            return self.fail(from, "/s_new", "node ID must be positive or -1");
        };

        // target is ignored in M3: everything hangs from the root group.
        let mut synth = Box::new(UGenSynth::new(Arc::clone(&def)));
        for pair in msg.args[4..].chunks(2) {
            let (Some(index), Some(value)) = (
                control_key(&pair[0], &def),
                pair.get(1).and_then(float_value),
            ) else {
                continue; // unknown controls are ignored, like scsynth
            };
            synth.set_control(index, value);
        }

        if self.handle.send(Cmd::AddSynth { id, synth, action }).is_ok() {
            self.node_defs.insert(id, def);
        } else {
            self.fail(from, "/s_new", "command FIFO full");
        }
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
fn control_key(arg: &OscType, def: &SynthDef) -> Option<u32> {
    match arg {
        OscType::String(name) => def.control_index(name),
        OscType::Int(i) if *i >= 0 => Some(*i as u32),
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
