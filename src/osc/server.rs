//! UDP OSC server implementing the M1 subset of the scsynth protocol:
//! `/status`, `/quit`, `/notify` and `/dumpOSC`.
//!
//! This runs on the network thread: allocating and doing I/O here is fine.
//! Replies follow scsynth semantics (see the `scsynth-osc` skill): `/status`
//! → `/status.reply`, asynchronous commands → `/done`, errors → `/fail`.

use std::io;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};

use rosc::{OscBundle, OscMessage, OscPacket, OscType, decoder, encoder};

/// Default scsynth port.
pub const DEFAULT_PORT: u16 = 57110;

/// Largest UDP datagram we accept.
const RECV_BUF_SIZE: usize = 65536;

/// Information reported in `/status.reply`. The counts are hardcoded to zero
/// until M2 wires the node tree in; the sample rates are real.
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
    dump_osc: bool,
    /// Clients registered via `/notify 1`; the client ID is index + 1.
    clients: Vec<SocketAddr>,
    recv_buf: Vec<u8>,
}

impl OscServer {
    pub fn bind(addr: impl ToSocketAddrs, info: ServerInfo) -> io::Result<Self> {
        Ok(Self {
            socket: UdpSocket::bind(addr)?,
            info,
            dump_osc: false,
            clients: Vec::new(),
            recv_buf: vec![0; RECV_BUF_SIZE],
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
            if let Flow::Quit = self.handle_packet(packet, from) {
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

    /// M1 executes bundle contents immediately; timetag scheduling is M6.
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
            "/dumpOSC" => {
                self.dump_osc = matches!(msg.args.first(), Some(OscType::Int(n)) if *n != 0);
            }
            "/quit" => {
                self.reply(from, "/done", vec![OscType::String("/quit".into())]);
                return Flow::Quit;
            }
            other => {
                self.reply(
                    from,
                    "/fail",
                    vec![
                        OscType::String(other.into()),
                        OscType::String("unknown command".into()),
                    ],
                );
            }
        }
        Flow::Continue
    }

    fn send_status(&mut self, to: SocketAddr) {
        let args = vec![
            OscType::Int(1),
            OscType::Int(0), // UGens
            OscType::Int(0), // synths
            OscType::Int(0), // groups
            OscType::Int(0), // loaded synthdefs
            OscType::Float(0.0), // avg CPU
            OscType::Float(0.0), // peak CPU
            OscType::Double(self.info.nominal_sample_rate),
            OscType::Double(self.info.actual_sample_rate),
        ];
        self.reply(to, "/status.reply", args);
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
            _ => {
                self.reply(
                    from,
                    "/fail",
                    vec![
                        OscType::String("/notify".into()),
                        OscType::String("expected int argument 0 or 1".into()),
                    ],
                );
            }
        }
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
