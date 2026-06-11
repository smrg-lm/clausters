//! F1 tests: the dedicated Faust compiler thread and the `/d_faust` OSC
//! round-trip with async replies. Gated behind the `faust` feature.
//! Tests wait on explicit completion signals (result channel, reply socket),
//! never on sleeps.

#![cfg(feature = "faust")]

use std::net::SocketAddr;
use std::time::Duration;

use clausters::faust::compiler::{CompilePayload, CompileRequest, CompilerThread};

/// Stdlib-free sine at 440 Hz: keeps the test independent of the Faust
/// library search path (stdlib imports are exercised from F2 on).
const SINE_SRC: &str = r#"
wrap(x) = x - floor(x);
phasor = (+(440.0/48000.0) : wrap) ~ _;
process = sin(6.283185307179586 * phasor) * 0.2;
"#;

fn dummy_client() -> SocketAddr {
    "127.0.0.1:1".parse().unwrap()
}

const COMPILE_DEADLINE: Duration = Duration::from_secs(10);

#[test]
fn compiler_thread_compiles_and_reports_back() {
    let compiler = CompilerThread::spawn();
    compiler
        .submit(CompileRequest {
            name: "sine".into(),
            payload: CompilePayload::Source(SINE_SRC.into()),
            client: dummy_client(),
        })
        .ok()
        .unwrap();

    let result = compiler
        .recv_result_timeout(COMPILE_DEADLINE)
        .expect("compilation must finish");
    assert_eq!(result.name, "sine");
    assert!(result.outcome.is_ok(), "{:?}", result.outcome.err());
}

#[test]
fn compiler_thread_reports_readable_errors() {
    let compiler = CompilerThread::spawn();
    compiler
        .submit(CompileRequest {
            name: "broken".into(),
            payload: CompilePayload::Source("process = nonsense(;".into()),
            client: dummy_client(),
        })
        .ok()
        .unwrap();

    let result = compiler
        .recv_result_timeout(COMPILE_DEADLINE)
        .expect("compilation must finish");
    let err = result.outcome.err().expect("broken source must fail");
    assert!(!err.is_empty(), "error must be human-readable");
}

#[test]
fn requests_are_serialized_in_order() {
    let compiler = CompilerThread::spawn();
    for name in ["a", "b", "c"] {
        compiler
            .submit(CompileRequest {
                name: name.into(),
                payload: CompilePayload::Source(SINE_SRC.into()),
                client: dummy_client(),
            })
            .ok()
            .unwrap();
    }
    for name in ["a", "b", "c"] {
        let result = compiler
            .recv_result_timeout(COMPILE_DEADLINE)
            .expect("compilation must finish");
        assert_eq!(result.name, name);
        assert!(result.outcome.is_ok());
    }
}

// ---- OSC round-trip ----

mod osc {
    use super::{COMPILE_DEADLINE, SINE_SRC};
    use std::net::UdpSocket;

    use clausters::osc::server::{OscServer, ServerInfo};
    use clausters::rosc::{OscMessage, OscPacket, OscType, decoder, encoder};
    use clausters::server::engine::engine_pair;

    struct TestServer {
        addr: std::net::SocketAddr,
        handle: std::thread::JoinHandle<std::io::Result<()>>,
        client: UdpSocket,
    }

    impl TestServer {
        fn spawn() -> Self {
            let (_engine, engine_handle) = engine_pair(48_000.0, 2);
            let info = ServerInfo {
                nominal_sample_rate: 48_000.0,
                actual_sample_rate: 48_000.0,
            };
            let mut server = OscServer::bind(("127.0.0.1", 0), info, engine_handle).unwrap();
            let addr = server.local_addr().unwrap();
            let handle = std::thread::spawn(move || server.run());
            let client = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
            client.set_read_timeout(Some(COMPILE_DEADLINE)).unwrap();
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
            self.client
                .send_to(&encoder::encode(&packet).unwrap(), self.addr)
                .unwrap();
        }

        fn recv_until(&self, addr: &str) -> OscMessage {
            let mut buf = [0u8; 65536];
            for _ in 0..100 {
                let (len, _) = self.client.recv_from(&mut buf).expect("reply timed out");
                if let (_, OscPacket::Message(msg)) = decoder::decode_udp(&buf[..len]).unwrap()
                    && msg.addr == addr
                {
                    return msg;
                }
            }
            panic!("never received {addr}");
        }

        fn quit(self) {
            self.send("/quit", vec![]);
            self.recv_until("/done");
            self.handle.join().unwrap().unwrap();
        }
    }

    #[test]
    fn d_faust_compiles_async_and_counts_the_def() {
        let server = TestServer::spawn();

        server.send(
            "/d_faust",
            vec![
                OscType::String("fsine".into()),
                OscType::String(SINE_SRC.into()),
            ],
        );
        // The reply is asynchronous: it arrives after the compiler thread
        // finishes and the server drains results (GC tick at the latest).
        let done = server.recv_until("/done");
        assert_eq!(done.args[0], OscType::String("/d_faust".into()));
        assert_eq!(done.args[1], OscType::String("fsine".into()));

        // /status def count includes the Faust table (1 built-in + 1 faust)
        server.send("/status", vec![]);
        let status = server.recv_until("/status.reply");
        assert_eq!(status.args[4], OscType::Int(2));

        // /d_free clears it
        server.send("/d_free", vec![OscType::String("fsine".into())]);
        server.send("/status", vec![]);
        let status = server.recv_until("/status.reply");
        assert_eq!(status.args[4], OscType::Int(1));

        server.quit();
    }

    #[test]
    fn d_faust_bad_source_fails_with_compiler_error() {
        let server = TestServer::spawn();
        server.send(
            "/d_faust",
            vec![
                OscType::String("broken".into()),
                OscType::String("process = nonsense(;".into()),
            ],
        );
        let fail = server.recv_until("/fail");
        assert_eq!(fail.args[0], OscType::String("/d_faust".into()));
        let OscType::String(why) = &fail.args[1] else {
            panic!("expected error string");
        };
        assert!(!why.is_empty());
        server.quit();
    }

    #[test]
    fn d_faust_json_payload_compiles() {
        let server = TestServer::spawn();
        server.send(
            "/d_faust",
            vec![
                OscType::String("jconst".into()),
                OscType::Blob(br#"{"op": "real", "value": 0.1}"#.to_vec()),
            ],
        );
        let done = server.recv_until("/done");
        assert_eq!(done.args[1], OscType::String("jconst".into()));
        server.quit();
    }

    #[test]
    fn d_faust_json_errors_carry_the_node_path() {
        let server = TestServer::spawn();
        server.send(
            "/d_faust",
            vec![
                OscType::String("jbad".into()),
                OscType::String(r#"{"op": "seq", "in": [{"op": "zzz"}, "_"]}"#.into()),
            ],
        );
        let fail = server.recv_until("/fail");
        let OscType::String(why) = &fail.args[1] else {
            panic!("expected error string");
        };
        assert!(why.contains("$.in[0]"), "error must locate the node: {why}");
        server.quit();
    }

    #[test]
    fn d_faust_bad_args_fail_immediately() {
        let server = TestServer::spawn();
        server.send("/d_faust", vec![OscType::Int(42)]);
        let fail = server.recv_until("/fail");
        assert_eq!(fail.args[0], OscType::String("/d_faust".into()));
        server.quit();
    }
}
