//! Integration tests for the M1 OSC server: ephemeral port, real UDP
//! round-trips, no audio device needed.

use std::net::{SocketAddr, UdpSocket};
use std::thread::JoinHandle;
use std::time::Duration;

use claudesufa::osc::server::{OscServer, ServerInfo};
use claudesufa::rosc::{OscMessage, OscPacket, OscType, decoder, encoder};

struct TestServer {
    addr: SocketAddr,
    handle: JoinHandle<std::io::Result<()>>,
    client: UdpSocket,
}

impl TestServer {
    fn spawn() -> Self {
        let info = ServerInfo {
            nominal_sample_rate: 48_000.0,
            actual_sample_rate: 48_000.0,
        };
        let mut server = OscServer::bind(("127.0.0.1", 0), info).unwrap();
        let addr = server.local_addr().unwrap();
        let handle = std::thread::spawn(move || server.run());

        let client = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        Self {
            addr,
            handle,
            client,
        }
    }

    fn send(&self, addr: &str, args: Vec<OscType>) {
        let packet = OscPacket::Message(OscMessage {
            addr: addr.into(),
            args,
        });
        let bytes = encoder::encode(&packet).unwrap();
        self.client.send_to(&bytes, self.addr).unwrap();
    }

    fn recv(&self) -> OscMessage {
        let mut buf = [0u8; 65536];
        let (len, _) = self.client.recv_from(&mut buf).expect("reply timed out");
        match decoder::decode_udp(&buf[..len]).unwrap().1 {
            OscPacket::Message(msg) => msg,
            OscPacket::Bundle(_) => panic!("expected a message, got a bundle"),
        }
    }

    fn quit(self) {
        self.send("/quit", vec![]);
        let reply = self.recv();
        assert_eq!(reply.addr, "/done");
        assert_eq!(reply.args[0], OscType::String("/quit".into()));
        self.handle.join().unwrap().unwrap();
    }
}

#[test]
fn status_reply_format() {
    let server = TestServer::spawn();
    server.send("/status", vec![]);
    let reply = server.recv();

    assert_eq!(reply.addr, "/status.reply");
    assert_eq!(reply.args.len(), 9);
    assert_eq!(reply.args[0], OscType::Int(1));
    // counts are zero until M2
    assert_eq!(reply.args[2], OscType::Int(0));
    assert_eq!(reply.args[7], OscType::Double(48_000.0));

    server.quit();
}

#[test]
fn notify_register_and_unregister() {
    let server = TestServer::spawn();

    server.send("/notify", vec![OscType::Int(1)]);
    let reply = server.recv();
    assert_eq!(reply.addr, "/done");
    assert_eq!(reply.args[0], OscType::String("/notify".into()));
    assert_eq!(reply.args[1], OscType::Int(1)); // first client gets ID 1

    // registering twice keeps the same ID
    server.send("/notify", vec![OscType::Int(1)]);
    assert_eq!(server.recv().args[1], OscType::Int(1));

    server.send("/notify", vec![OscType::Int(0)]);
    assert_eq!(server.recv().addr, "/done");

    server.quit();
}

#[test]
fn notify_bad_argument_fails() {
    let server = TestServer::spawn();
    server.send("/notify", vec![OscType::String("yes".into())]);
    assert_eq!(server.recv().addr, "/fail");
    server.quit();
}

#[test]
fn unknown_command_fails() {
    let server = TestServer::spawn();
    server.send("/does_not_exist", vec![]);
    let reply = server.recv();
    assert_eq!(reply.addr, "/fail");
    assert_eq!(reply.args[0], OscType::String("/does_not_exist".into()));
    server.quit();
}

#[test]
fn bundle_contents_execute() {
    use claudesufa::rosc::OscBundle;

    let server = TestServer::spawn();
    let bundle = OscPacket::Bundle(OscBundle {
        timetag: rosc_immediate(),
        content: vec![OscPacket::Message(OscMessage {
            addr: "/status".into(),
            args: vec![],
        })],
    });
    let bytes = encoder::encode(&bundle).unwrap();
    server.client.send_to(&bytes, server.addr).unwrap();
    assert_eq!(server.recv().addr, "/status.reply");
    server.quit();
}

fn rosc_immediate() -> claudesufa::rosc::OscTime {
    claudesufa::rosc::OscTime {
        seconds: 0,
        fractional: 1,
    }
}
