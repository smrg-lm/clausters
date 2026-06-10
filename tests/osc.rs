//! Integration tests for the OSC server: ephemeral port, real UDP
//! round-trips, no audio device needed. The engine is ticked manually from
//! the test (manual clock), never in real time.

use std::net::{SocketAddr, UdpSocket};
use std::thread::JoinHandle;
use std::time::Duration;

use claudesufa::osc::server::{OscServer, ServerInfo};
use claudesufa::rosc::{OscMessage, OscPacket, OscType, decoder, encoder};
use claudesufa::server::engine::{BLOCK_SIZE, Engine, engine_pair};

struct TestServer {
    addr: SocketAddr,
    handle: JoinHandle<std::io::Result<()>>,
    client: UdpSocket,
    engine: Engine,
}

impl TestServer {
    fn spawn() -> Self {
        let (engine, engine_handle) = engine_pair(48_000.0, 2);
        let info = ServerInfo {
            nominal_sample_rate: 48_000.0,
            actual_sample_rate: 48_000.0,
        };
        let mut server = OscServer::bind(("127.0.0.1", 0), info, engine_handle).unwrap();
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
            engine,
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

    /// Receives until a message with this address arrives, discarding others
    /// (e.g. interleaved /n_go//n_end notifications).
    fn recv_until(&self, addr: &str) -> OscMessage {
        for _ in 0..100 {
            let msg = self.recv();
            if msg.addr == addr {
                return msg;
            }
        }
        panic!("never received {addr}");
    }

    /// Ticks the engine and polls /status until the given reply argument
    /// matches or the deadline passes. Covers the network→FIFO→audio round
    /// trip. Argument 2 is the synth count, 3 the group count.
    fn wait_for_status(&mut self, arg_index: usize, expected: i32) {
        let mut out = vec![0.0f32; BLOCK_SIZE * 2];
        for _ in 0..100 {
            self.engine.process_block(&mut out);
            self.send("/status", vec![]);
            if self.recv_until("/status.reply").args[arg_index] == OscType::Int(expected) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("status arg {arg_index} never reached {expected}");
    }

    fn wait_for_synth_count(&mut self, expected: i32) {
        self.wait_for_status(2, expected);
    }

    /// Ticks the engine, nudging the server with /status, until a message
    /// with this address arrives (used for /n_go and /n_end, whose timing
    /// depends on the engine applying the command first).
    fn tick_until(&mut self, addr: &str) -> OscMessage {
        let mut out = vec![0.0f32; BLOCK_SIZE * 2];
        for _ in 0..100 {
            self.engine.process_block(&mut out);
            self.send("/status", vec![]);
            let msg = self.recv();
            if msg.addr == addr {
                return msg;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("never received {addr}");
    }

    fn quit(self) {
        self.send("/quit", vec![]);
        let reply = self.recv_until("/done");
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
    assert_eq!(reply.args[2], OscType::Int(0)); // no synths yet
    assert_eq!(reply.args[3], OscType::Int(1)); // root group
    assert_eq!(reply.args[4], OscType::Int(1)); // the built-in "default" def
    assert_eq!(reply.args[7], OscType::Double(48_000.0));

    server.quit();
}

#[test]
fn d_recv_compiles_def_and_plays_it() {
    let mut server = TestServer::spawn();
    let json = r#"{
        "name": "beep2",
        "controls": [{"name": "freq", "default": 220.0}],
        "ugens": [
            {"kind": "SinOsc", "inputs": [{"control": 0}]},
            {"kind": "Mul",    "inputs": [{"ugen": 0}, {"const": 0.1}]},
            {"kind": "Out",    "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]
    }"#;
    server.send("/d_recv", vec![OscType::Blob(json.as_bytes().to_vec())]);
    let reply = server.recv();
    assert_eq!(reply.addr, "/done");
    assert_eq!(reply.args[0], OscType::String("/d_recv".into()));

    server.send(
        "/s_new",
        vec![
            OscType::String("beep2".into()),
            OscType::Int(1000),
            OscType::Int(1),
            OscType::Int(0),
        ],
    );
    server.wait_for_synth_count(1);

    // /n_set by name resolves through the def mirror (no /fail expected)
    server.send(
        "/n_set",
        vec![
            OscType::Int(1000),
            OscType::String("freq".into()),
            OscType::Float(330.0),
        ],
    );

    server.send("/n_free", vec![OscType::Int(1000)]);
    server.wait_for_synth_count(0);
    server.quit();
}

#[test]
fn d_recv_invalid_json_fails() {
    let server = TestServer::spawn();
    server.send("/d_recv", vec![OscType::Blob(b"not json".to_vec())]);
    let reply = server.recv();
    assert_eq!(reply.addr, "/fail");
    assert_eq!(reply.args[0], OscType::String("/d_recv".into()));
    server.quit();
}

#[test]
fn d_recv_bad_graph_fails_with_compile_error() {
    let server = TestServer::spawn();
    let json = r#"{"name":"x","ugens":[{"kind":"Nope","inputs":[]}]}"#;
    server.send("/d_recv", vec![OscType::String(json.into())]);
    let reply = server.recv();
    assert_eq!(reply.addr, "/fail");
    let OscType::String(why) = &reply.args[1] else {
        panic!("expected error string");
    };
    assert!(why.contains("unknown kind"), "{why}");
    server.quit();
}

#[test]
fn d_free_removes_def() {
    let server = TestServer::spawn();
    let json = r#"{"name":"temp","ugens":[{"kind":"SinOsc","inputs":[{"const":440.0}]}]}"#;
    server.send("/d_recv", vec![OscType::String(json.into())]);
    assert_eq!(server.recv().addr, "/done");

    server.send("/status", vec![]);
    assert_eq!(server.recv().args[4], OscType::Int(2)); // default + temp

    server.send("/d_free", vec![OscType::String("temp".into())]);
    server.send("/status", vec![]);
    assert_eq!(server.recv().args[4], OscType::Int(1));

    // s_new on the freed def now fails
    server.send(
        "/s_new",
        vec![
            OscType::String("temp".into()),
            OscType::Int(1000),
            OscType::Int(1),
            OscType::Int(0),
        ],
    );
    assert_eq!(server.recv().addr, "/fail");
    server.quit();
}

#[test]
fn n_set_unknown_node_fails() {
    let server = TestServer::spawn();
    server.send(
        "/n_set",
        vec![
            OscType::Int(4242),
            OscType::String("freq".into()),
            OscType::Float(100.0),
        ],
    );
    let reply = server.recv();
    assert_eq!(reply.addr, "/fail");
    assert_eq!(reply.args[0], OscType::String("/n_set".into()));
    server.quit();
}

#[test]
fn s_new_and_n_free_update_status_counts() {
    let mut server = TestServer::spawn();

    server.send(
        "/s_new",
        vec![
            OscType::String("default".into()),
            OscType::Int(1000),
            OscType::Int(1),
            OscType::Int(0),
            OscType::String("freq".into()),
            OscType::Float(330.0),
        ],
    );
    server.wait_for_synth_count(1);

    server.send("/n_free", vec![OscType::Int(1000)]);
    server.wait_for_synth_count(0);

    server.quit();
}

#[test]
fn s_new_unknown_synthdef_fails() {
    let server = TestServer::spawn();
    server.send(
        "/s_new",
        vec![
            OscType::String("nonexistent".into()),
            OscType::Int(1000),
            OscType::Int(1),
            OscType::Int(0),
        ],
    );
    let reply = server.recv();
    assert_eq!(reply.addr, "/fail");
    assert_eq!(reply.args[0], OscType::String("/s_new".into()));
    server.quit();
}

#[test]
fn g_new_and_free_update_group_count() {
    let mut server = TestServer::spawn();

    server.send(
        "/g_new",
        vec![OscType::Int(1), OscType::Int(1), OscType::Int(0)],
    );
    server.wait_for_status(3, 2); // root + new group

    server.send("/n_free", vec![OscType::Int(1)]);
    server.wait_for_status(3, 1);

    server.quit();
}

#[test]
fn g_free_all_empties_group_but_keeps_it() {
    let mut server = TestServer::spawn();

    server.send(
        "/g_new",
        vec![OscType::Int(1), OscType::Int(1), OscType::Int(0)],
    );
    server.wait_for_status(3, 2);

    // a synth inside the group (addAction tail of group 1)
    server.send(
        "/s_new",
        vec![
            OscType::String("default".into()),
            OscType::Int(1000),
            OscType::Int(1),
            OscType::Int(1),
        ],
    );
    server.wait_for_synth_count(1);

    server.send("/g_freeAll", vec![OscType::Int(1)]);
    server.wait_for_synth_count(0);
    server.wait_for_status(3, 2); // the group itself survives

    server.quit();
}

#[test]
fn c_set_and_c_get_roundtrip() {
    let server = TestServer::spawn();

    server.send(
        "/c_set",
        vec![OscType::Int(5), OscType::Float(0.25)],
    );
    server.send("/c_get", vec![OscType::Int(5)]);
    let reply = server.recv_until("/c_set");
    assert_eq!(reply.args[0], OscType::Int(5));
    assert_eq!(reply.args[1], OscType::Float(0.25));

    // unset buses read as 0.0
    server.send("/c_get", vec![OscType::Int(99)]);
    let reply = server.recv_until("/c_set");
    assert_eq!(reply.args[1], OscType::Float(0.0));

    server.quit();
}

#[test]
fn notify_clients_receive_n_go_and_n_end() {
    let mut server = TestServer::spawn();
    server.send("/notify", vec![OscType::Int(1)]);
    assert_eq!(server.recv_until("/done").args[1], OscType::Int(1));

    server.send(
        "/s_new",
        vec![
            OscType::String("default".into()),
            OscType::Int(1000),
            OscType::Int(1),
            OscType::Int(0),
            OscType::String("amp".into()),
            OscType::Float(0.0),
        ],
    );
    let go = server.tick_until("/n_go");
    assert_eq!(go.args[0], OscType::Int(1000));
    assert_eq!(go.args[1], OscType::Int(0)); // parent: root group
    assert_eq!(go.args[4], OscType::Int(0)); // not a group

    server.send("/n_free", vec![OscType::Int(1000)]);
    let end = server.tick_until("/n_end");
    assert_eq!(end.args[0], OscType::Int(1000));
    assert_eq!(end.args[4], OscType::Int(0));

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
    use claudesufa::rosc::{OscBundle, OscTime};

    let server = TestServer::spawn();
    let bundle = OscPacket::Bundle(OscBundle {
        timetag: OscTime {
            seconds: 0,
            fractional: 1,
        },
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
