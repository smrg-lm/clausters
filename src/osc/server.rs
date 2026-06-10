//! UDP OSC server implementing the M2 subset of the scsynth protocol:
//! `/status`, `/quit`, `/notify`, `/dumpOSC`, `/s_new`, `/n_free`, `/n_set`.
//!
//! This runs on the network thread: allocating and doing I/O here is fine.
//! It owns the [`EngineHandle`]: node commands are fully built here (boxed
//! synth included) and pushed to the engine's command FIFO; garbage coming
//! back from the audio thread is dropped here. Replies follow scsynth
//! semantics (see the `scsynth-osc` skill).

use std::io;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::Ordering;
use std::time::Duration;

use rosc::{OscBundle, OscMessage, OscPacket, OscType, decoder, encoder};

use crate::node::AddAction;
use crate::node::default_synth::{DefaultSynth, control_index};
use crate::server::engine::{Cmd, EngineHandle};

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
        Ok(Self {
            socket,
            info,
            handle,
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
                    self.handle.collect_garbage();
                    continue;
                }
                // A previous send to a now-closed client port can surface as
                // ECONNREFUSED on the next recv (Linux); not fatal.
                Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => continue,
                Err(e) => return Err(e),
            };
            let packet = match decoder::decode_udp(&self.recv_buf[..len]) {
                Ok((_, packet)) => packet,
                Err(e) => {
                    eprintln!("malformed OSC packet from {from}: {e}");
                    continue;
                }
            };
            let flow = self.handle_packet(packet, from);
            self.handle.collect_garbage();
            if let Flow::Quit = flow {
                return Ok(());
            }
        }
    }

    fn handle_packet(&mut self, packet: OscPacket, from: SocketAddr) -> Flow {
        match packet {
            OscPacket::Message(msg) => self.handle_message(msg, from),
            OscPacket::Bundle(bundle) => self.handle_bundle(bundle, from),
        }
    }

    /// M2 executes bundle contents immediately; timetag scheduling is M6.
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
            OscType::Int(0), // loaded synthdefs: M3
            OscType::Float(0.0), // avg CPU
            OscType::Float(0.0), // peak CPU
            OscType::Double(self.info.nominal_sample_rate),
            OscType::Double(self.info.actual_sample_rate),
        ];
        self.reply(to, "/status.reply", args);
    }

    fn handle_s_new(&mut self, msg: &OscMessage, from: SocketAddr) {
        let (def, id, action) = match msg.args.as_slice() {
            [
                OscType::String(def),
                OscType::Int(id),
                OscType::Int(action),
                OscType::Int(_target),
                ..,
            ] => (def.clone(), *id, *action),
            _ => return self.fail(from, "/s_new", "expected: name, id, addAction, targetID"),
        };
        if def != "default" {
            return self.fail(from, "/s_new", format!("SynthDef not found: {def}"));
        }
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

        // target is ignored in M2: everything hangs from the root group.
        let mut synth = Box::new(DefaultSynth::new(440.0, 0.2));
        for pair in msg.args[4..].chunks(2) {
            let (Some(index), Some(value)) = (control_key(&pair[0]), pair.get(1).and_then(float_value)) else {
                continue; // unknown controls are ignored, like scsynth
            };
            synth.set_control(index, value);
        }

        if self.handle.send(Cmd::AddSynth { id, synth, action }).is_err() {
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
        for pair in msg.args[1..].chunks(2) {
            let (Some(index), Some(value)) = (control_key(&pair[0]), pair.get(1).and_then(float_value)) else {
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

/// Control reference: by name (`"freq"`) or by index (`0`).
fn control_key(arg: &OscType) -> Option<u32> {
    match arg {
        OscType::String(name) => control_index(name),
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
