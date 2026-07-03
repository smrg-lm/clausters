//! Integration tests for the OSC server: ephemeral port, real UDP
//! round-trips, no audio device needed. The engine is ticked manually from
//! the test (manual clock), never in real time.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, UdpSocket};
use std::thread::JoinHandle;
use std::time::Duration;

use clausters::osc::server::{OscServer, ServerInfo};
use clausters::rosc::{OscMessage, OscPacket, OscType, decoder, encoder};
use clausters::server::engine::{BLOCK_SIZE, Engine, engine_pair};

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
fn n_run_pauses_a_node_and_rejects_unknown() {
    let mut server = TestServer::spawn();

    server.send(
        "/s_new",
        vec![
            OscType::String("default".into()),
            OscType::Int(1000),
            OscType::Int(1),
            OscType::Int(0),
        ],
    );
    server.wait_for_synth_count(1);

    // Pause then resume the node over the immediate (UDP) path: each is
    // accepted with no reply (a following /status.reply arrives before any
    // /fail — this guards the dispatch wiring, since a command that fell
    // through to the default arm would answer /fail first), and the node stays
    // in the tree (a paused node is not freed, so the count is unchanged).
    server.send("/n_run", vec![OscType::Int(1000), OscType::Int(0)]);
    server.send("/status", vec![]);
    assert_eq!(
        server.recv().addr,
        "/status.reply",
        "valid /n_run must not /fail"
    );
    server.send("/n_run", vec![OscType::Int(1000), OscType::Int(1)]);
    server.send("/status", vec![]);
    assert_eq!(
        server.recv().addr,
        "/status.reply",
        "valid /n_run must not /fail"
    );
    server.wait_for_synth_count(1);

    // An unknown node fails.
    server.send("/n_run", vec![OscType::Int(9999), OscType::Int(0)]);
    let reply = server.recv();
    assert_eq!(reply.addr, "/fail");
    assert_eq!(reply.args[0], OscType::String("/n_run".into()));

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

    server.send("/c_set", vec![OscType::Int(5), OscType::Float(0.25)]);
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
fn b_getn_and_b_get_read_buffer_samples() {
    let server = TestServer::spawn();

    // A 6-frame mono WAV with known samples, loaded into buffer 0.
    let samples: Vec<f32> = vec![0.0, 0.1, 0.2, 0.3, 0.4, 0.5];
    let path = std::env::temp_dir().join(format!("clausters_b_getn_{}.wav", std::process::id()));
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for s in &samples {
        w.write_sample(*s).unwrap();
    }
    w.finalize().unwrap();

    server.send(
        "/b_allocRead",
        vec![
            OscType::Int(0),
            OscType::String(path.to_str().unwrap().into()),
        ],
    );
    // Wait for the async load to install the buffer.
    server.recv_until("/done");

    // A range read, asking for more than the buffer holds: count clamps to 6.
    server.send(
        "/b_getn",
        vec![OscType::Int(0), OscType::Int(0), OscType::Int(100)],
    );
    let reply = server.recv_until("/b_setn");
    assert_eq!(reply.args[0], OscType::Int(0)); // bufnum
    assert_eq!(reply.args[1], OscType::Int(0)); // start
    assert_eq!(reply.args[2], OscType::Int(6)); // count, clamped
    let got: Vec<f32> = reply.args[3..]
        .iter()
        .map(|a| match a {
            OscType::Float(f) => *f,
            other => panic!("expected float, got {other:?}"),
        })
        .collect();
    assert_eq!(got, samples);

    // A mid-range slice.
    server.send(
        "/b_getn",
        vec![OscType::Int(0), OscType::Int(2), OscType::Int(3)],
    );
    let reply = server.recv_until("/b_setn");
    assert_eq!(reply.args[1], OscType::Int(2));
    assert_eq!(reply.args[2], OscType::Int(3));
    // args = [bufnum, start, count, samples[2], samples[3], samples[4]]
    assert_eq!(reply.args[3], OscType::Float(0.2));
    assert_eq!(reply.args[5], OscType::Float(0.4));

    // Indexed reads; an out-of-range index reads as 0.0.
    server.send(
        "/b_get",
        vec![OscType::Int(0), OscType::Int(3), OscType::Int(99)],
    );
    let reply = server.recv_until("/b_set");
    assert_eq!(reply.args[0], OscType::Int(0));
    assert_eq!(reply.args[1], OscType::Int(3));
    assert_eq!(reply.args[2], OscType::Float(0.3));
    assert_eq!(reply.args[3], OscType::Int(99));
    assert_eq!(reply.args[4], OscType::Float(0.0));

    // An unallocated buffer yields an empty range (count 0), not an error.
    server.send(
        "/b_getn",
        vec![OscType::Int(7), OscType::Int(0), OscType::Int(4)],
    );
    let reply = server.recv_until("/b_setn");
    assert_eq!(reply.args[0], OscType::Int(7));
    assert_eq!(reply.args[2], OscType::Int(0));

    let _ = std::fs::remove_file(&path);
    server.quit();
}

#[test]
fn b_gen_fills_a_wavetable_then_reads_it_back() {
    use clausters::dsp::wavetable::wt_interp;
    let server = TestServer::spawn();

    // A 256-sample buffer = a 128-point wavetable.
    server.send("/b_alloc", vec![OscType::Int(0), OscType::Int(256)]);
    server.recv_until("/done");

    // Fill it with a single sine partial, wavetable format, normalized+cleared.
    server.send(
        "/b_gen",
        vec![
            OscType::Int(0),
            OscType::String("sine1".into()),
            OscType::Int(1 + 2 + 4),
            OscType::Float(1.0),
        ],
    );
    let done = server.recv_until("/done");
    assert_eq!(done.args[0], OscType::String("/b_gen".into()));
    assert_eq!(done.args[1], OscType::Int(0));

    // Read the whole table back and confirm it reconstructs the sine.
    server.send(
        "/b_getn",
        vec![OscType::Int(0), OscType::Int(0), OscType::Int(256)],
    );
    let reply = server.recv_until("/b_setn");
    assert_eq!(reply.args[2], OscType::Int(256));
    let table: Vec<f32> = reply.args[3..]
        .iter()
        .map(|a| match a {
            OscType::Float(f) => *f,
            other => panic!("expected float, got {other:?}"),
        })
        .collect();
    let points = table.len() / 2;
    for k in 0..points {
        let expect = (std::f32::consts::TAU * k as f32 / points as f32).sin();
        assert!(
            (wt_interp(&table, k, 0.0) - expect).abs() < 1e-3,
            "point {k}"
        );
    }

    // An unknown generator and an unallocated target both /fail.
    server.send(
        "/b_gen",
        vec![
            OscType::Int(0),
            OscType::String("bogus".into()),
            OscType::Int(0),
        ],
    );
    assert_eq!(
        server.recv_until("/fail").args[0],
        OscType::String("/b_gen".into())
    );
    server.send(
        "/b_gen",
        vec![
            OscType::Int(9),
            OscType::String("sine1".into()),
            OscType::Int(7),
            OscType::Float(1.0),
        ],
    );
    assert_eq!(
        server.recv_until("/fail").args[0],
        OscType::String("/b_gen".into())
    );

    server.quit();
}

#[test]
fn b_export_dumps_raw_samples_to_a_local_file() {
    let server = TestServer::spawn();

    // A 6-frame mono WAV loaded into buffer 0.
    let samples: Vec<f32> = vec![0.0, 0.1, 0.2, 0.3, 0.4, 0.5];
    let wav =
        std::env::temp_dir().join(format!("clausters_b_export_src_{}.wav", std::process::id()));
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(&wav, spec).unwrap();
    for s in &samples {
        w.write_sample(*s).unwrap();
    }
    w.finalize().unwrap();
    server.send(
        "/b_allocRead",
        vec![
            OscType::Int(0),
            OscType::String(wav.to_str().unwrap().into()),
        ],
    );
    server.recv_until("/done");

    // Export the buffer to a raw little-endian f32 file (the bulk shared-resource
    // path) and confirm the /done names the command and buffer.
    let out =
        std::env::temp_dir().join(format!("clausters_b_export_out_{}.f32", std::process::id()));
    server.send(
        "/b_export",
        vec![
            OscType::Int(0),
            OscType::String(out.to_str().unwrap().into()),
        ],
    );
    let done = server.recv_until("/done");
    assert_eq!(done.args[0], OscType::String("/b_export".into()));
    assert_eq!(done.args[1], OscType::Int(0));

    // The file is exactly the samples as little-endian f32 (what the GUI host maps).
    let bytes = std::fs::read(&out).unwrap();
    let got: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    assert_eq!(got, samples);

    // Exporting an unallocated buffer fails (and writes no file).
    server.send(
        "/b_export",
        vec![
            OscType::Int(7),
            OscType::String(out.to_str().unwrap().into()),
        ],
    );
    assert_eq!(
        server.recv_until("/fail").args[0],
        OscType::String("/b_export".into())
    );

    let _ = std::fs::remove_file(&wav);
    let _ = std::fs::remove_file(&out);
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
    use clausters::rosc::{OscBundle, OscTime};

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

/// M8/M21: `/clock` exposes the engine's sample counter, the actual sample
/// rate and the server's OSC/NTP time captured with the counter — the anchor a
/// client needs to place its clock on the server's sample axis.
#[test]
fn clock_reports_the_engine_sample_counter() {
    let mut server = TestServer::spawn();

    server.send("/clock", vec![]);
    let reply = server.recv_until("/clock.reply");
    assert_eq!(reply.args[0], OscType::Long(0), "fresh engine starts at 0");
    assert_eq!(reply.args[1], OscType::Double(48_000.0));
    // The third field is the master-clock anchor: the server's OSC time, which
    // must be within a few seconds of the test's own wall clock.
    match reply.args[2] {
        OscType::Time(t) => {
            let secs = t.seconds as f64 - 2_208_988_800.0 + t.fractional as f64 / 2f64.powi(32);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs_f64();
            assert!(
                (secs - now).abs() < 5.0,
                "osc_time {secs} not near now {now}"
            );
        }
        ref other => panic!("expected an OSC timetag as the third arg, got {other:?}"),
    }

    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    for _ in 0..10 {
        server.engine.process_block(&mut out);
    }
    server.send("/clock", vec![]);
    let reply = server.recv_until("/clock.reply");
    assert_eq!(reply.args[0], OscType::Long(10 * BLOCK_SIZE as i64));
    server.quit();
}

/// M22: `/transport` is the shared beat grid for phase alignment — a query
/// reports "undefined" until a client sets it, then echoes it back; bad args
/// fail and leave the previous grid intact.
#[test]
fn transport_query_and_set() {
    let server = TestServer::spawn();

    // Unset: defined flag 0, zeros. Reply is (origin, tempo, defined, playing,
    // position) -- the grid plus the rolling state.
    server.send("/transport", vec![]);
    let reply = server.recv_until("/transport.reply");
    assert_eq!(
        reply.args,
        vec![
            OscType::Long(0),
            OscType::Double(0.0),
            OscType::Int(0),
            OscType::Int(0),
            OscType::Double(0.0),
        ]
    );

    // Set origin sample + tempo; replies /done.
    server.send(
        "/transport",
        vec![OscType::Long(96_000), OscType::Double(2.0)],
    );
    assert_eq!(
        server.recv_until("/done").args[0],
        OscType::String("/transport".into())
    );

    // Query now reports the grid with defined 1, stopped at position 0.
    server.send("/transport", vec![]);
    let reply = server.recv_until("/transport.reply");
    assert_eq!(
        reply.args,
        vec![
            OscType::Long(96_000),
            OscType::Double(2.0),
            OscType::Int(1),
            OscType::Int(0),
            OscType::Double(0.0),
        ]
    );

    // Bad tempo fails and does not clobber the stored grid.
    server.send("/transport", vec![OscType::Long(0), OscType::Double(0.0)]);
    assert_eq!(
        server.recv_until("/fail").args[0],
        OscType::String("/transport".into())
    );
    server.send("/transport", vec![]);
    assert_eq!(
        server.recv_until("/transport.reply").args[2],
        OscType::Int(1)
    );

    server.quit();
}

/// The DAW-style rolling state: play / stop / locate update `playing` and
/// `position`, reply `/done`, and need a grid defined first.
#[test]
fn transport_play_stop_locate() {
    let server = TestServer::spawn();

    // play/stop/locate before a grid is defined -> /fail.
    server.send("/transport_play", vec![]);
    assert_eq!(
        server.recv_until("/fail").args[0],
        OscType::String("/transport_play".into())
    );

    server.send("/transport", vec![OscType::Long(0), OscType::Double(2.0)]);
    server.recv_until("/done");

    // Play from beat 8: playing=1, position=8.
    server.send("/transport_play", vec![OscType::Double(8.0)]);
    server.recv_until("/done");
    server.send("/transport", vec![]);
    let reply = server.recv_until("/transport.reply");
    assert_eq!(reply.args[3], OscType::Int(1)); // playing
    assert_eq!(reply.args[4], OscType::Double(8.0)); // position

    // Locate to 16 while playing: position moves, playing unchanged.
    server.send("/transport_locate", vec![OscType::Double(16.0)]);
    server.recv_until("/done");
    server.send("/transport", vec![]);
    let reply = server.recv_until("/transport.reply");
    assert_eq!(reply.args[3], OscType::Int(1));
    assert_eq!(reply.args[4], OscType::Double(16.0));

    // Stop: playing=0, position holds.
    server.send("/transport_stop", vec![]);
    server.recv_until("/done");
    server.send("/transport", vec![]);
    let reply = server.recv_until("/transport.reply");
    assert_eq!(reply.args[3], OscType::Int(0));
    assert_eq!(reply.args[4], OscType::Double(16.0));

    server.quit();
}

/// A `/notify` client is pushed the new grid as a `/transport.reply` whenever
/// the transport is set, so its responders re-align without polling (M22
/// push-on-change paired with client responders).
#[test]
fn transport_pushes_on_change_to_notify_clients() {
    let server = TestServer::spawn();

    server.send("/notify", vec![OscType::Int(1)]);
    assert_eq!(server.recv_until("/done").args[1], OscType::Int(1));

    // Setting the transport replies /done to the setter and pushes the grid.
    server.send(
        "/transport",
        vec![OscType::Long(48_000), OscType::Double(2.0)],
    );
    let push = server.recv_until("/transport.reply");
    assert_eq!(
        push.args,
        vec![
            OscType::Long(48_000),
            OscType::Double(2.0),
            OscType::Int(1),
            OscType::Int(0),
            OscType::Double(0.0),
        ]
    );

    // A play also pushes the rolling state to the /notify client.
    server.send("/transport_play", vec![OscType::Double(4.0)]);
    let push = server.recv_until("/transport.reply");
    assert_eq!(push.args[3], OscType::Int(1));
    assert_eq!(push.args[4], OscType::Double(4.0));

    server.quit();
}

/// M8: `/sched` argument validation and per-message translation failures.
#[test]
fn sched_rejects_bad_arguments() {
    let server = TestServer::spawn();
    let s_new_blob = || {
        encoder::encode(&OscPacket::Message(OscMessage {
            addr: "/s_new".into(),
            args: vec![
                OscType::String("default".into()),
                OscType::Int(1000),
                OscType::Int(1),
                OscType::Int(0),
            ],
        }))
        .unwrap()
    };

    // No arguments at all.
    server.send("/sched", vec![]);
    assert_eq!(
        server.recv_until("/fail").args[0],
        OscType::String("/sched".into())
    );
    // Target without a packet blob.
    server.send("/sched", vec![OscType::Long(100)]);
    assert_eq!(
        server.recv_until("/fail").args[0],
        OscType::String("/sched".into())
    );
    // Negative target.
    server.send(
        "/sched",
        vec![OscType::Long(-1), OscType::Blob(s_new_blob())],
    );
    assert_eq!(
        server.recv_until("/fail").args[0],
        OscType::String("/sched".into())
    );
    // Garbage blob.
    server.send(
        "/sched",
        vec![OscType::Long(100), OscType::Blob(vec![1, 2, 3, 4])],
    );
    assert_eq!(
        server.recv_until("/fail").args[0],
        OscType::String("/sched".into())
    );
    // A query is not schedulable: the /fail names the offending message.
    let status_blob = encoder::encode(&OscPacket::Message(OscMessage {
        addr: "/status".into(),
        args: vec![],
    }))
    .unwrap();
    server.send(
        "/sched",
        vec![OscType::Long(100), OscType::Blob(status_blob)],
    );
    assert_eq!(
        server.recv_until("/fail").args[0],
        OscType::String("/status".into())
    );
    server.quit();
}

/// M8: an `Int` target is tolerated and the blob may be a bundle — all its
/// leaf messages fire as one atomic instant (inner timetags are ignored).
#[test]
fn sched_accepts_int_targets_and_bundle_blobs() {
    use clausters::rosc::{OscBundle, OscTime};

    let mut server = TestServer::spawn();
    let bundle = OscPacket::Bundle(OscBundle {
        // A far-future NTP tag that must be ignored: /sched is the clock.
        timetag: OscTime {
            seconds: u32::MAX,
            fractional: 0,
        },
        content: vec![OscPacket::Message(OscMessage {
            addr: "/s_new".into(),
            args: vec![
                OscType::String("default".into()),
                OscType::Int(1000),
                OscType::Int(1),
                OscType::Int(0),
            ],
        })],
    });
    server.send(
        "/sched",
        vec![
            OscType::Int(64),
            OscType::Blob(encoder::encode(&bundle).unwrap()),
        ],
    );
    // If the inner NTP tag were honored, this synth would never start.
    server.wait_for_synth_count(1);
    server.quit();
}

// ---- TCP transport (server track M / client C8) ----

/// A length-prefixed OSC client over TCP: a 4-byte big-endian length then the
/// OSC bytes, the same framing the server's `osc::tcp` speaks both ways.
struct TcpClient {
    stream: TcpStream,
}

impl TcpClient {
    fn connect(addr: SocketAddr) -> Self {
        let stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        Self { stream }
    }

    fn send(&mut self, addr: &str, args: Vec<OscType>) {
        let bytes = encoder::encode(&OscPacket::Message(OscMessage {
            addr: addr.into(),
            args,
        }))
        .unwrap();
        self.stream
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .unwrap();
        self.stream.write_all(&bytes).unwrap();
    }

    fn recv(&mut self) -> OscMessage {
        let mut prefix = [0u8; 4];
        self.stream
            .read_exact(&mut prefix)
            .expect("reply timed out");
        let len = u32::from_be_bytes(prefix) as usize;
        let mut buf = vec![0u8; len];
        self.stream
            .read_exact(&mut buf)
            .expect("short framed reply");
        match decoder::decode_udp(&buf).unwrap().1 {
            OscPacket::Message(msg) => msg,
            OscPacket::Bundle(_) => panic!("expected a message, got a bundle"),
        }
    }

    fn recv_until(&mut self, addr: &str) -> OscMessage {
        for _ in 0..100 {
            let msg = self.recv();
            if msg.addr == addr {
                return msg;
            }
        }
        panic!("never received {addr}");
    }
}

/// Spawns a server with TCP enabled (no audio device); returns the TCP address,
/// the join handle and the engine kept alive so the handle stays valid.
fn spawn_tcp_server() -> (SocketAddr, JoinHandle<std::io::Result<()>>, Engine) {
    let (engine, handle) = engine_pair(48_000.0, 2);
    let info = ServerInfo {
        nominal_sample_rate: 48_000.0,
        actual_sample_rate: 48_000.0,
    };
    let mut server = OscServer::bind(("127.0.0.1", 0), info, handle).unwrap();
    let tcp_addr = server.listen_tcp(("127.0.0.1", 0)).unwrap();
    let join = std::thread::spawn(move || server.run());
    (tcp_addr, join, engine)
}

#[test]
fn tcp_status_and_d_recv_roundtrip() {
    let (tcp_addr, _join, _engine) = spawn_tcp_server();
    let mut client = TcpClient::connect(tcp_addr);

    // A query round-trips over the framed connection (the zero-length-UDP wake
    // means we do not wait for the GC tick).
    client.send("/status", vec![]);
    assert_eq!(client.recv_until("/status.reply").addr, "/status.reply");

    // A SynthDef sent over TCP compiles and the async /done comes back framed
    // on the same connection.
    let spec = br#"{"name":"tcp_def","controls":[],"ugens":[{"kind":"WhiteNoise","inputs":[]},{"kind":"Out","inputs":[{"const":0.0},{"ugen":0}]}]}"#;
    client.send("/d_recv", vec![OscType::Blob(spec.to_vec())]);
    assert_eq!(
        client.recv_until("/done").args[0],
        OscType::String("/d_recv".into())
    );
}

#[test]
fn tcp_replies_route_to_the_originating_connection() {
    let (tcp_addr, _join, _engine) = spawn_tcp_server();
    let mut a = TcpClient::connect(tcp_addr);
    let mut b = TcpClient::connect(tcp_addr);

    // Only `a` asks: only `a` must receive the reply (per-connection routing).
    a.send("/status", vec![]);
    assert_eq!(a.recv_until("/status.reply").addr, "/status.reply");

    // `b` is still healthy on its own connection afterwards.
    b.send("/status", vec![]);
    assert_eq!(b.recv_until("/status.reply").addr, "/status.reply");
}

#[test]
fn sync_answers_synced_with_the_same_id() {
    let server = TestServer::spawn();
    // Nothing async outstanding: /synced comes back immediately, echoing the id.
    server.send("/sync", vec![OscType::Int(42)]);
    let reply = server.recv_until("/synced");
    assert_eq!(reply.args, vec![OscType::Int(42)]);
    server.quit();
}

#[test]
fn sync_waits_for_an_async_buffer_alloc() {
    let server = TestServer::spawn();
    // Queue an async buffer alloc (runs on the NRT thread), then the barrier.
    server.send(
        "/b_alloc",
        vec![OscType::Int(0), OscType::Int(64), OscType::Int(1)],
    );
    server.send("/sync", vec![OscType::Int(7)]);

    // The barrier must not answer before the alloc's /done lands.
    let mut saw_done = false;
    for _ in 0..100 {
        let msg = server.recv();
        if msg.addr == "/done" && msg.args.first() == Some(&OscType::String("/b_alloc".into())) {
            saw_done = true;
        }
        if msg.addr == "/synced" {
            assert_eq!(msg.args, vec![OscType::Int(7)]);
            assert!(saw_done, "/synced arrived before the buffer's /done");
            server.quit();
            return;
        }
    }
    panic!("never received /synced");
}
