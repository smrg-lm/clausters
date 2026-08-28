//! Integration tests for the OSC server: ephemeral port, real UDP
//! round-trips, no audio device needed. The engine is ticked manually from
//! the test (manual clock), never in real time.

#![cfg(feature = "synth")]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, UdpSocket};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use clausters::dsp::Limits;
use clausters::osc::server::{OscServer, ServerInfo};
use clausters::rosc::{OscMessage, OscPacket, OscType, decoder, encoder};

/// The samples in a `/buffer_getRange.reply` range: one little-endian f32 blob
/// (docs/schemas.md, "bulk samples travel as a blob").
fn range_samples(arg: &OscType) -> Vec<f32> {
    match arg {
        OscType::Blob(bytes) => bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect(),
        other => panic!("expected a sample blob, got {other:?}"),
    }
}
use clausters::server::engine::{BLOCK_SIZE, Engine, engine_pair, engine_pair_full};
use clausters::server::ipc::Segment;

struct TestServer {
    addr: SocketAddr,
    handle: JoinHandle<std::io::Result<()>>,
    client: UdpSocket,
    engine: Engine,
}

impl TestServer {
    fn spawn() -> Self {
        Self::spawn_with(engine_pair(48_000.0, 2))
    }

    /// A server over an engine built with explicit boot-time [`Limits`] (S7),
    /// to check `/server_query` reports the configured capacities.
    fn spawn_with_limits(limits: Limits) -> Self {
        Self::spawn_with(engine_pair_full(48_000.0, 2, 0, None, 128, 1024, limits))
    }

    fn spawn_with(pair: (Engine, clausters::server::engine::EngineHandle)) -> Self {
        let (engine, engine_handle) = pair;
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
    /// (e.g. interleaved /node_start//node_end notifications).
    fn recv_until(&self, addr: &str) -> OscMessage {
        for _ in 0..100 {
            let msg = self.recv();
            if msg.addr == addr {
                return msg;
            }
        }
        panic!("never received {addr}");
    }

    /// Waits briefly for a message with this address, returning `None` when
    /// none arrives — for asserting that something is **not** sent, which a
    /// blocking receive cannot do.
    fn try_recv_matching(&self, addr: &str) -> Option<OscMessage> {
        self.try_recv_within(addr, Duration::from_millis(120))
    }

    /// `try_recv_matching` with the wait spelled out — for a caller that has
    /// to keep the engine running while it waits, and so cannot afford to
    /// block for the default.
    fn try_recv_within(&self, addr: &str, wait: Duration) -> Option<OscMessage> {
        self.client.set_read_timeout(Some(wait)).unwrap();
        let mut buf = [0u8; 65536];
        let found = loop {
            let Ok((len, _)) = self.client.recv_from(&mut buf) else {
                break None;
            };
            if let Ok((_, OscPacket::Message(msg))) = decoder::decode_udp(&buf[..len])
                && msg.addr == addr
            {
                break Some(msg);
            }
        };
        self.client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        found
    }

    /// A `/server_sync` round trip, **ticking the engine while it waits**:
    /// what a caller does before assuming an asynchronous buffer command has
    /// landed. The engine has to run for the command FIFO to be drained, and
    /// in this harness the test is the audio thread.
    fn sync(&mut self) {
        self.send("/server_sync", vec![OscType::Int(4242)]);
        let mut out = vec![0.0f32; BLOCK_SIZE * 2];
        for _ in 0..200 {
            self.engine.process_block(&mut out);
            if self.try_recv_matching("/server_sync.reply").is_some() {
                return;
            }
        }
        panic!("the server never answered /server_sync.reply");
    }

    /// Collects the `addr` replies of a multi-reply query (M30's `/def_query`,
    /// `/ugen_query`) until the batch's `/done` terminator arrives.
    fn recv_batch(&self, addr: &str, cmd: &str) -> Vec<OscMessage> {
        let mut out = Vec::new();
        for _ in 0..500 {
            let msg = self.recv();
            if msg.addr == "/done" && msg.args.first() == Some(&OscType::String(cmd.into())) {
                return out;
            }
            if msg.addr == addr {
                out.push(msg);
            }
        }
        panic!("never received the /done terminating {cmd}");
    }

    /// Ticks the engine and polls /server_status until the given reply argument
    /// matches or the deadline passes. Covers the network→FIFO→audio round
    /// trip. The reply opens on its first real field, so argument 1 is the
    /// synth count and 2 the group count — reached through the two named
    /// wrappers below rather than by index.
    fn wait_for_status(&mut self, arg_index: usize, expected: i32) {
        let mut out = vec![0.0f32; BLOCK_SIZE * 2];
        for _ in 0..100 {
            self.engine.process_block(&mut out);
            self.send("/server_status", vec![]);
            if self.recv_until("/server_status.reply").args[arg_index] == OscType::Int(expected) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("status arg {arg_index} never reached {expected}");
    }

    fn wait_for_synth_count(&mut self, expected: i32) {
        self.wait_for_status(1, expected);
    }

    /// The same poll on the group count, named so a test never spells the
    /// field's position.
    fn wait_for_group_count(&mut self, expected: i32) {
        self.wait_for_status(2, expected);
    }

    /// Ticks the engine, nudging the server with /server_status, until a message
    /// with this address arrives (used for /node_start and /node_end, whose timing
    /// depends on the engine applying the command first).
    fn tick_until(&mut self, addr: &str) -> OscMessage {
        let mut out = vec![0.0f32; BLOCK_SIZE * 2];
        for _ in 0..100 {
            self.engine.process_block(&mut out);
            self.send("/server_status", vec![]);
            let msg = self.recv();
            if msg.addr == addr {
                return msg;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("never received {addr}");
    }

    fn quit(self) {
        self.send("/server_quit", vec![]);
        let reply = self.recv_until("/done");
        assert_eq!(reply.args[0], OscType::String("/server_quit".into()));
        self.handle.join().unwrap().unwrap();
    }
}

#[test]
fn status_reply_format() {
    let server = TestServer::spawn();
    server.send("/server_status", vec![]);
    let reply = server.recv();

    assert_eq!(reply.addr, "/server_status.reply");
    // The 8 scsynth-shaped fields it kept -- the leading unused int is not one
    // of them -- plus the appended late-block counter.
    assert_eq!(reply.args.len(), 9);
    assert_eq!(reply.args[1], OscType::Int(0)); // no synths yet
    assert_eq!(reply.args[2], OscType::Int(1)); // root group
    assert_eq!(reply.args[3], OscType::Int(1)); // the built-in "default" def
    // avg/peak CPU are real measurements (percent, non-negative and finite).
    for cpu in [&reply.args[4], &reply.args[5]] {
        match cpu {
            OscType::Float(v) => assert!(*v >= 0.0 && v.is_finite(), "cpu = {v}"),
            other => panic!("cpu fields must be floats, got {other:?}"),
        }
    }
    assert_eq!(reply.args[6], OscType::Double(48_000.0));
    assert!(matches!(reply.args[8], OscType::Int(n) if n >= 0)); // late blocks

    server.quit();
}

#[test]
fn d_recv_compiles_def_and_plays_it() {
    let mut server = TestServer::spawn();
    let json = r#"{
        "name": "beep2",
        "controls": [{"name": "freq", "default": 220.0}],
        "ugens": [
            {"kind": "Sine", "inputs": [{"control": 0}]},
            {"kind": "Mul",    "inputs": [{"ugen": 0}, {"const": 0.1}]},
            {"kind": "Out",    "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]
    }"#;
    server.send(
        "/def_send",
        vec![
            OscType::String("synth".into()),
            OscType::Blob(json.as_bytes().to_vec()),
        ],
    );
    let reply = server.recv();
    assert_eq!(reply.addr, "/done");
    assert_eq!(reply.args[0], OscType::String("/def_send".into()));

    server.send(
        "/synth_new",
        vec![
            OscType::String("beep2".into()),
            OscType::Int(1000),
            OscType::Int(1),
            OscType::Int(0),
        ],
    );
    server.wait_for_synth_count(1);

    // /node_set by name resolves through the def mirror (no /fail expected)
    server.send(
        "/node_set",
        vec![
            OscType::Int(1000),
            OscType::String("freq".into()),
            OscType::Float(330.0),
        ],
    );

    server.send("/node_free", vec![OscType::Int(1000)]);
    server.wait_for_synth_count(0);
    server.quit();
}

#[test]
fn d_recv_invalid_json_fails() {
    let server = TestServer::spawn();
    server.send(
        "/def_send",
        vec![
            OscType::String("synth".into()),
            OscType::Blob(b"not json".to_vec()),
        ],
    );
    let reply = server.recv();
    assert_eq!(reply.addr, "/fail");
    assert_eq!(reply.args[0], OscType::String("/def_send".into()));
    server.quit();
}

#[test]
fn d_recv_bad_graph_fails_with_compile_error() {
    let server = TestServer::spawn();
    let json = r#"{"name":"x","ugens":[{"kind":"Nope","inputs":[]}]}"#;
    server.send(
        "/def_send",
        vec![
            OscType::String("synth".into()),
            OscType::String(json.into()),
        ],
    );
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
    let json = r#"{"name":"temp","ugens":[{"kind":"Sine","inputs":[{"const":440.0}]}]}"#;
    server.send(
        "/def_send",
        vec![
            OscType::String("synth".into()),
            OscType::String(json.into()),
        ],
    );
    assert_eq!(server.recv().addr, "/done");

    server.send("/server_status", vec![]);
    assert_eq!(server.recv().args[3], OscType::Int(2)); // default + temp

    server.send("/def_free", vec![OscType::String("temp".into())]);
    server.send("/server_status", vec![]);
    assert_eq!(server.recv().args[3], OscType::Int(1));

    // s_new on the freed def now fails
    server.send(
        "/synth_new",
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
        "/node_set",
        vec![
            OscType::Int(4242),
            OscType::String("freq".into()),
            OscType::Float(100.0),
        ],
    );
    let reply = server.recv();
    assert_eq!(reply.addr, "/fail");
    assert_eq!(reply.args[0], OscType::String("/node_set".into()));
    server.quit();
}

#[test]
fn s_new_and_n_free_update_status_counts() {
    let mut server = TestServer::spawn();

    server.send(
        "/synth_new",
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

    server.send("/node_free", vec![OscType::Int(1000)]);
    server.wait_for_synth_count(0);

    server.quit();
}

#[test]
fn n_run_pauses_a_node_and_rejects_unknown() {
    let mut server = TestServer::spawn();

    server.send(
        "/synth_new",
        vec![
            OscType::String("default".into()),
            OscType::Int(1000),
            OscType::Int(1),
            OscType::Int(0),
        ],
    );
    server.wait_for_synth_count(1);

    // Pause then resume the node over the immediate (UDP) path: each is
    // accepted with no reply (a following /server_status.reply arrives before any
    // /fail — this guards the dispatch wiring, since a command that fell
    // through to the default arm would answer /fail first), and the node stays
    // in the tree (a paused node is not freed, so the count is unchanged).
    server.send("/node_run", vec![OscType::Int(1000), OscType::Int(0)]);
    server.send("/server_status", vec![]);
    assert_eq!(
        server.recv().addr,
        "/server_status.reply",
        "valid /node_run must not /fail"
    );
    server.send("/node_run", vec![OscType::Int(1000), OscType::Int(1)]);
    server.send("/server_status", vec![]);
    assert_eq!(
        server.recv().addr,
        "/server_status.reply",
        "valid /node_run must not /fail"
    );
    server.wait_for_synth_count(1);

    // An unknown node fails.
    server.send("/node_run", vec![OscType::Int(9999), OscType::Int(0)]);
    let reply = server.recv();
    assert_eq!(reply.addr, "/fail");
    assert_eq!(reply.args[0], OscType::String("/node_run".into()));

    server.quit();
}

#[test]
fn s_new_unknown_synthdef_fails() {
    let server = TestServer::spawn();
    server.send(
        "/synth_new",
        vec![
            OscType::String("nonexistent".into()),
            OscType::Int(1000),
            OscType::Int(1),
            OscType::Int(0),
        ],
    );
    let reply = server.recv();
    assert_eq!(reply.addr, "/fail");
    assert_eq!(reply.args[0], OscType::String("/synth_new".into()));
    server.quit();
}

#[test]
fn g_new_and_free_update_group_count() {
    let mut server = TestServer::spawn();

    server.send(
        "/group_new",
        vec![OscType::Int(1), OscType::Int(1), OscType::Int(0)],
    );
    server.wait_for_group_count(2); // root + new group

    server.send("/node_free", vec![OscType::Int(1)]);
    server.wait_for_group_count(1);

    server.quit();
}

#[test]
fn g_free_all_empties_group_but_keeps_it() {
    let mut server = TestServer::spawn();

    server.send(
        "/group_new",
        vec![OscType::Int(1), OscType::Int(1), OscType::Int(0)],
    );
    server.wait_for_group_count(2);

    // a synth inside the group (addAction tail of group 1)
    server.send(
        "/synth_new",
        vec![
            OscType::String("default".into()),
            OscType::Int(1000),
            OscType::Int(1),
            OscType::Int(1),
        ],
    );
    server.wait_for_synth_count(1);

    server.send("/group_freeAll", vec![OscType::Int(1)]);
    server.wait_for_synth_count(0);
    server.wait_for_group_count(2); // the group itself survives

    server.quit();
}

#[test]
fn c_set_and_c_get_roundtrip() {
    let server = TestServer::spawn();

    server.send("/bus_set", vec![OscType::Int(5), OscType::Float(0.25)]);
    server.send("/bus_get", vec![OscType::Int(5)]);
    let reply = server.recv_until("/bus_get.reply");
    assert_eq!(reply.args[0], OscType::Int(5));
    assert_eq!(reply.args[1], OscType::Float(0.25));

    // unset buses read as 0.0
    server.send("/bus_get", vec![OscType::Int(99)]);
    let reply = server.recv_until("/bus_get.reply");
    assert_eq!(reply.args[1], OscType::Float(0.0));

    server.quit();
}

#[test]
fn c_stream_acks_snapshots_and_tracks_updates() {
    let server = TestServer::spawn();

    server.send("/bus_set", vec![OscType::Int(3), OscType::Float(0.5)]);
    server.send(
        "/bus_stream",
        vec![OscType::Int(20), OscType::Int(3), OscType::Int(7)],
    );
    let done = server.recv_until("/done");
    assert_eq!(done.args[0], OscType::String("/bus_stream".into()));

    // The immediate snapshot carries (busIndex, value) pairs for both buses.
    let snap = server.recv_until("/bus_stream.reply");
    assert_eq!(
        snap.args,
        vec![
            OscType::Int(3),
            OscType::Float(0.5),
            OscType::Int(7),
            OscType::Float(0.0),
        ]
    );

    // The stream keeps coming without any further request, and tracks writes.
    server.send("/bus_set", vec![OscType::Int(7), OscType::Float(0.75)]);
    let mut saw_update = false;
    for _ in 0..20 {
        let frame = server.recv_until("/bus_stream.reply");
        assert_eq!(frame.args.len(), 4, "one (index, value) pair per bus");
        if frame.args[3] == OscType::Float(0.75) {
            saw_update = true;
            break;
        }
    }
    assert!(saw_update, "the periodic snapshots never showed the write");

    server.quit();
}

#[test]
fn c_stream_resubscribe_replaces_and_zero_cancels() {
    let server = TestServer::spawn();

    server.send("/bus_stream", vec![OscType::Int(20), OscType::Int(1)]);
    server.recv_until("/done");
    server.recv_until("/bus_stream.reply");

    // A second subscription replaces the first: frames now carry bus 2 only.
    server.send("/bus_stream", vec![OscType::Int(20), OscType::Int(2)]);
    server.recv_until("/done");
    for _ in 0..20 {
        let frame = server.recv_until("/bus_stream.reply");
        if frame.args[0] == OscType::Int(2) {
            assert_eq!(frame.args.len(), 2, "the old subscription must be gone");
            break;
        }
    }

    // Period 0 cancels: after the ack and a drain, the stream is silent.
    server.send("/bus_stream", vec![OscType::Int(0)]);
    server.recv_until("/done");
    server
        .client
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let mut buf = [0u8; 65536];
    loop {
        // Drain frames already in flight; three periods of silence ends it.
        if server.client.recv_from(&mut buf).is_err() {
            break;
        }
    }
    assert!(
        server.client.recv_from(&mut buf).is_err(),
        "cancelled stream kept sending"
    );

    server
        .client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    server.quit();
}

#[test]
fn c_stream_rejects_bad_arguments() {
    let server = TestServer::spawn();

    server.send("/bus_stream", vec![]);
    let reply = server.recv_until("/fail");
    assert_eq!(reply.args[0], OscType::String("/bus_stream".into()));

    server.send("/bus_stream", vec![OscType::Int(20), OscType::Int(-1)]);
    let reply = server.recv_until("/fail");
    assert_eq!(reply.args[0], OscType::String("/bus_stream".into()));

    // Over what one reply can carry to *this* client -- a datagram-bounded one
    // keeps the overview budget, so about eight hundred pairs. The refusal
    // names the number, which is what a client with no `/server_query` in hand
    // has to go on.
    let mut args = vec![OscType::Int(1000)];
    args.extend((0..4096).map(OscType::Int));
    server.send("/bus_stream", args);
    let reply = server.recv_until("/fail");
    assert_eq!(reply.args[0], OscType::String("/bus_stream".into()));
    let OscType::String(why) = &reply.args[1] else {
        panic!("expected a reason, got {:?}", reply.args[1]);
    };
    assert!(
        why.starts_with("at most ") && why.ends_with("on this carrier"),
        "the refusal has to name the ceiling: {why}"
    );

    server.quit();
}

/// The ceiling is a boot-time configuration and not the shape of a widget: a
/// page holding many canvases subscribes the union of what they draw, so a
/// subscription grows with the document. 128 was the old constant, and it is
/// the number a page of sixty-four canvases walked into.
#[test]
fn c_stream_carries_far_more_than_a_frame_of_meters() {
    let server = TestServer::spawn();

    // Slow enough that the snapshots do not flood the socket before the ack.
    let mut args = vec![OscType::Int(1000)];
    args.extend((0..400).map(OscType::Int));
    server.send("/bus_stream", args);
    let done = server.recv_until("/done");
    assert_eq!(done.args[0], OscType::String("/bus_stream".into()));

    let snap = server.recv_until("/bus_stream.reply");
    assert_eq!(snap.args.len(), 800, "400 (index, value) pairs");

    // No cancel: the subscription dies with the server, and the `/done` a
    // cancel would ack is the reply `quit()` is waiting for.
    server.quit();
}

/// **The overview of a buffer that is standing still**, which is
/// `/buffer_stream`'s sibling: a recording's summary is pushed as it is
/// written, and a buffer nothing writes has one that is simply asked for.
///
/// What the test pins is the pair of things a client depends on: the blob is
/// the *same* layout the stream reports (bucket-major, channel-minor, min, max
/// and mean square), so the folding half does not fork; and the conversation is
/// `/buffer_getRange`'s — one reply per request, its own length saying how much
/// came, the start rounded down to a whole bucket so the grids agree.
#[test]
fn buffer_peaks_answers_the_overview_of_a_buffer_at_rest() {
    let mut server = TestServer::spawn();

    // A stereo buffer of four buckets: the left channel a constant, the right
    // one silent, so a bucket says which channel it is.
    server.send(
        "/buffer_alloc",
        vec![OscType::Int(3), OscType::Int(1_024), OscType::Int(2)],
    );
    server.sync();
    let mut samples: Vec<OscType> = vec![OscType::Int(3), OscType::Int(0)];
    let mut blob = Vec::new();
    for _ in 0..1_024 {
        blob.extend_from_slice(&0.5f32.to_le_bytes());
        blob.extend_from_slice(&0.0f32.to_le_bytes());
    }
    samples.push(OscType::Blob(blob));
    server.send("/buffer_setRange", samples);
    server.sync();

    server.send("/buffer_peaks", vec![OscType::Int(3), OscType::Int(256)]);
    let reply = server.recv_until("/buffer_peaks.reply");
    assert_eq!(reply.args[0], OscType::Int(3));
    assert_eq!(reply.args[1], OscType::Long(0));
    assert_eq!(reply.args[2], OscType::Int(256));
    let OscType::Blob(bytes) = &reply.args[3] else {
        panic!("expected the overview blob, got {:?}", reply.args);
    };
    let values: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    assert_eq!(
        values.len(),
        4 * 2 * 3,
        "four buckets, two channels, three each"
    );
    for frame in values.as_chunks::<6>().0 {
        assert_eq!((frame[0], frame[1]), (0.5, 0.5), "left is the constant");
        assert!((frame[2] - 0.25).abs() < 1e-6, "its energy");
        assert_eq!(
            (frame[3], frame[4], frame[5]),
            (0.0, 0.0, 0.0),
            "right is silent"
        );
    }

    // A span, and a start that is not on the grid: it is rounded **down**, so
    // what comes back can be folded into a summary bucket for bucket.
    server.send(
        "/buffer_peaks",
        vec![
            OscType::Int(3),
            OscType::Int(256),
            OscType::Int(300),
            OscType::Int(512),
        ],
    );
    let reply = server.recv_until("/buffer_peaks.reply");
    assert_eq!(
        reply.args[1],
        OscType::Long(256),
        "rounded down to the grid"
    );
    let OscType::Blob(bytes) = &reply.args[3] else {
        panic!("expected the overview blob");
    };
    assert_eq!(
        bytes.len(),
        2 * 2 * 3 * 4,
        "frames 256..812 hold two buckets"
    );

    // **A reply is bounded in bytes, not in buckets**, because a bucket is not
    // a size: the smallest carrier is the shared ring at 64 KiB and it drops
    // what does not fit in silence. So a long take answers in several replies,
    // each saying where it began, and the walk continues from there.
    server.send(
        "/buffer_alloc",
        vec![OscType::Int(4), OscType::Int(4_000_000), OscType::Int(2)],
    );
    server.sync();
    server.send("/buffer_peaks", vec![OscType::Int(4), OscType::Int(256)]);
    let reply = server.recv_until("/buffer_peaks.reply");
    let OscType::Blob(bytes) = &reply.args[3] else {
        panic!("expected the overview blob");
    };
    assert!(
        bytes.len() <= 8 * 1024,
        "one reply fits the smallest carrier, got {} bytes",
        bytes.len()
    );
    assert!(!bytes.is_empty(), "and it is not empty either");

    // A span with no whole bucket in it answers empty rather than not at all:
    // the asker learns the span held none, which is not the same as silence.
    server.send(
        "/buffer_peaks",
        vec![
            OscType::Int(3),
            OscType::Int(256),
            OscType::Int(0),
            OscType::Int(100),
        ],
    );
    let reply = server.recv_until("/buffer_peaks.reply");
    assert_eq!(reply.args[3], OscType::Blob(Vec::new()));

    server.quit();
}

/// **A server with no segment reports a recording too**, which is the case the
/// command exists for: whoever cannot map the samples is who asks for its
/// overview, and most of them talk to a server that shares nothing — an engine
/// inside a page, a `clausters` booted without `--shm`.
///
/// It read the write frontier out of the segment's directory row, so exactly
/// there it reported nothing at all, forever and silently. The frontier is the
/// **buffer's** now, and the row is where a mapping peer reads it.
#[test]
fn buffer_stream_reports_a_recording_with_no_segment_behind_it() {
    let mut server = TestServer::spawn();

    // A recorder writing a constant into a buffer of its own: the frontier
    // moves because a writing UGen moved it, which is the only thing that does.
    server.send(
        "/buffer_alloc",
        vec![OscType::Int(2), OscType::Int(4_096), OscType::Int(1)],
    );
    server.sync();
    let json = r#"{
        "name": "recorder",
        "ugens": [
            {"kind": "Line", "inputs": [
                {"const": 0.5}, {"const": 0.5}, {"const": 100.0}, {"const": 0.0}
            ]},
            {"kind": "RecordBuf", "inputs": [
                {"const": 2.0}, {"const": 0.0}, {"ugen": 0},
                {"const": 0.0}, {"const": 1.0}, {"const": 0.0},
                {"const": 1.0}, {"const": 0.0}, {"const": 0.0}, {"const": 0.0}
            ]}
        ]
    }"#;
    server.send(
        "/def_send",
        vec![
            OscType::String("synth".into()),
            OscType::Blob(json.as_bytes().to_vec()),
        ],
    );
    server.recv_until("/done");

    server.send(
        "/buffer_stream",
        vec![OscType::Int(20), OscType::Int(256), OscType::Int(2)],
    );
    let done = server.recv_until("/done");
    assert_eq!(done.args[0], OscType::String("/buffer_stream".into()));

    server.send(
        "/synth_new",
        vec![
            OscType::String("recorder".into()),
            OscType::Int(1000),
            OscType::Int(1),
            OscType::Int(0),
        ],
    );
    server.wait_for_synth_count(1);

    // Enough blocks to fill several buckets, and enough time for one report.
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    for _ in 0..40 {
        server.engine.process_block(&mut out);
    }
    let reply = server.recv_until("/buffer_stream.reply");
    assert_eq!(reply.args[0], OscType::Int(2));
    assert_eq!(
        reply.args[1],
        OscType::Long(0),
        "it starts at the beginning"
    );
    let OscType::Blob(bytes) = &reply.args[3] else {
        panic!("expected the overview blob, got {:?}", reply.args);
    };
    let values: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    assert!(!values.is_empty(), "a recording that ran reports buckets");
    for bucket in values.as_chunks::<3>().0 {
        assert_eq!((bucket[0], bucket[1]), (0.5, 0.5), "the recorded constant");
        assert!((bucket[2] - 0.25).abs() < 1e-6, "its energy: 0.5 squared");
    }

    server.send("/node_free", vec![OscType::Int(1000)]);
    server.quit();
}

/// `/bus_tap` routes a live bus into a segment tap ring and `/bus_tapStream` streams
/// windows of it: ack, immediate snapshot (index + stream position + raw LE
/// `f32` blob), audible content, cancel, and the /fail cases.
#[test]
fn buffer_stream_reports_the_overview_of_what_was_recorded() {
    let segment = Segment::in_memory_full(1024, 0, 0);
    let mut server = TestServer::spawn_with(engine_pair_full(
        48_000.0,
        2,
        0,
        Some(Arc::clone(&segment)),
        128,
        1024,
        Limits::default(),
    ));

    // A published row is what a stream watches: the buffer's shape, and the
    // frontier its writers move. Publishing it here stands in for the
    // allocation path, which needs a segment on disk to share a region.
    segment
        .publish_buffer(7, 4_096, 1, 48_000.0)
        .expect("a row");
    server.send(
        "/buffer_stream",
        vec![OscType::Int(20), OscType::Int(256), OscType::Int(7)],
    );
    let done = server.recv_until("/done");
    assert_eq!(done.args[0], OscType::String("/buffer_stream".into()));

    // Nothing has been written, so nothing is reported: a still buffer costs
    // no traffic, which is what makes a subscription per take affordable.
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    for _ in 0..8 {
        server.engine.process_block(&mut out);
    }
    assert!(
        server.try_recv_matching("/buffer_stream.reply").is_none(),
        "an unwritten buffer reports nothing"
    );

    // The samples appears and the frontier says how far it goes. (In a real
    // recording the writing UGen does both; here the two halves are done by
    // hand so the test is about the stream and not about a synth.)
    server.send(
        "/buffer_alloc",
        vec![OscType::Int(7), OscType::Int(4_096), OscType::Int(1)],
    );
    server.sync();
    let run: Vec<u8> = (0..1_024)
        .flat_map(|i| (if i % 2 == 0 { 0.5f32 } else { -0.5 }).to_le_bytes())
        .collect();
    server.send(
        "/buffer_setRange",
        vec![OscType::Int(7), OscType::Int(0), OscType::Blob(run)],
    );
    server.sync();
    segment.raise_buffer_frontier(7, 1_024);

    for _ in 0..8 {
        server.engine.process_block(&mut out);
    }
    let reply = server.recv_until("/buffer_stream.reply");
    assert_eq!(reply.args[0], OscType::Int(7));
    assert_eq!(
        reply.args[1],
        OscType::Long(0),
        "it starts where it left off"
    );
    assert_eq!(reply.args[2], OscType::Int(256));
    let OscType::Blob(bytes) = &reply.args[3] else {
        panic!("expected the overview blob, got {:?}", reply.args);
    };
    // Four buckets, one channel, three statistics each, four bytes apiece.
    assert_eq!(bytes.len(), 4 * 3 * 4);
    let values: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    for bucket in values.as_chunks::<3>().0 {
        assert_eq!((bucket[0], bucket[1]), (-0.5, 0.5), "min and max");
        assert!(
            (bucket[2] - 0.25).abs() < 1e-6,
            "mean square: {}",
            bucket[2]
        );
    }

    // A report carries what is **new**: nothing moved, so nothing follows.
    for _ in 0..8 {
        server.engine.process_block(&mut out);
    }
    assert!(
        server.try_recv_matching("/buffer_stream.reply").is_none(),
        "a frontier that did not move reports nothing"
    );

    // Period 0 cancels, acked like the other two streams.
    server.send("/buffer_stream", vec![OscType::Int(0), OscType::Int(256)]);
    let done = server.recv_until("/done");
    assert_eq!(done.args[0], OscType::String("/buffer_stream".into()));
}

#[test]
fn tap_and_tap_stream_snapshot_audio() {
    let segment = Segment::in_memory_full(1024, 2, 4096);
    let mut server = TestServer::spawn_with(engine_pair_full(
        48_000.0,
        2,
        0,
        Some(segment),
        128,
        1024,
        Limits::default(),
    ));

    // Ask to watch audio bus 0 -- the server picks the ring (no ack; failures
    // reply /fail) -- then give it something to record: the built-in default
    // synth on bus 0.
    server.send("/bus_tap", vec![OscType::Int(0), OscType::Int(1)]);
    server.send(
        "/synth_new",
        vec![
            OscType::String("default".into()),
            OscType::Int(1000),
            OscType::Int(1),
            OscType::Int(0),
        ],
    );
    server.wait_for_synth_count(1);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    for _ in 0..16 {
        server.engine.process_block(&mut out);
    }

    // Subscribe: /done, then the immediate /bus_tapStream.reply snapshot.
    server.send(
        "/bus_tapStream",
        vec![OscType::Int(50), OscType::Int(512), OscType::Int(0)],
    );
    let done = server.recv_until("/done");
    assert_eq!(done.args[0], OscType::String("/bus_tapStream".into()));
    let data = server.recv_until("/bus_tapStream.reply");
    assert_eq!(data.args[0], OscType::Int(0));
    let OscType::Long(end) = data.args[1] else {
        panic!(
            "expected the stream position as a Long, got {:?}",
            data.args
        );
    };
    assert!(end >= 512, "at least one window written (end = {end})");
    let OscType::Blob(bytes) = &data.args[2] else {
        panic!("expected the window blob, got {:?}", data.args);
    };
    assert_eq!(bytes.len(), 512 * 4, "512 raw little-endian f32 samples");
    let peak = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c).abs())
        .fold(0.0f32, f32::max);
    assert!(peak > 0.01, "tapped audio must not be silent (peak {peak})");

    // Period 0 cancels (acked); an out-of-range bus fails, in both
    // directions -- watching it and releasing it are equally impossible.
    server.send("/bus_tapStream", vec![OscType::Int(0), OscType::Int(512)]);
    let done = server.recv_until("/done");
    assert_eq!(done.args[0], OscType::String("/bus_tapStream".into()));
    server.send("/bus_tap", vec![OscType::Int(999), OscType::Int(1)]);
    let fail = server.recv_until("/fail");
    assert_eq!(fail.args[0], OscType::String("/bus_tap".into()));
    server.send("/bus_tap", vec![OscType::Int(999), OscType::Int(0)]);
    let fail = server.recv_until("/fail");
    assert_eq!(fail.args[0], OscType::String("/bus_tap".into()));

    // Two rings, three buses: the third watch has nowhere to land and says so
    // rather than silently drawing nothing.
    server.send("/bus_tap", vec![OscType::Int(1), OscType::Int(1)]);
    server.send("/bus_tap", vec![OscType::Int(2), OscType::Int(1)]);
    let fail = server.recv_until("/fail");
    assert_eq!(fail.args[0], OscType::String("/bus_tap".into()));
    // Releasing one frees its ring for the next watcher.
    server.send("/bus_tap", vec![OscType::Int(1), OscType::Int(0)]);
    server.send("/bus_tap", vec![OscType::Int(2), OscType::Int(1)]);
    server.send("/server_status", vec![]);
    let reply = server.recv_until("/server_status.reply");
    assert_eq!(
        reply.addr, "/server_status.reply",
        "no /fail followed the retry"
    );

    server.quit();
}

/// The per-bus levels: published for every audio bus with no tap involved,
/// and **held with a decay** so a reader slower than the engine — a display
/// frame is a dozen blocks — sees a transient instead of missing it between
/// looks. The hold is a decay rather than a max the reader clears, so several
/// readers of one bus all see it.
#[test]
fn bus_levels_are_published_for_every_bus_and_held_with_a_decay() {
    let segment = Segment::in_memory_full(1024, 2, 4096);
    let mut server = TestServer::spawn_with(engine_pair_full(
        48_000.0,
        2,
        0,
        Some(Arc::clone(&segment)),
        128,
        1024,
        Limits::default(),
    ));

    // Silence reads as silence, on a bus nothing has ever touched.
    assert_eq!(segment.level(0), 0.0);
    assert_eq!(segment.level(64), 0.0);

    server.send(
        "/synth_new",
        vec![
            OscType::String("default".into()),
            OscType::Int(1000),
            OscType::Int(1),
            OscType::Int(0),
        ],
    );
    server.wait_for_synth_count(1);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    for _ in 0..64 {
        server.engine.process_block(&mut out);
    }
    let sounding = segment.level(0);
    assert!(sounding > 0.01, "a sounding bus meters (level {sounding})");
    // No /bus_tap was ever sent: metering costs no ring.
    assert_eq!(segment.tap_of_bus(0), None);

    // Silence the source and watch the hold decay rather than drop: a frame's
    // worth of blocks later the peak is still legible, which is the whole
    // point of publishing a held value instead of the raw block peak.
    server.send("/node_free", vec![OscType::Int(1000)]);
    server.wait_for_synth_count(0);
    server.engine.process_block(&mut out);
    let after_one = segment.level(0);
    assert!(
        after_one > sounding * 0.9,
        "one silent block barely moves it ({after_one} vs {sounding})"
    );
    for _ in 0..16 {
        server.engine.process_block(&mut out);
    }
    let after_frame = segment.level(0);
    assert!(
        after_frame < after_one && after_frame > sounding * 0.5,
        "a display frame later it has decayed but is still readable \
         ({after_frame} vs {sounding})"
    );
    for _ in 0..4000 {
        server.engine.process_block(&mut out);
    }
    assert!(
        segment.level(0) < sounding * 0.01,
        "and it does fall away: {}",
        segment.level(0)
    );

    server.quit();
}

/// A server without a tap region (no segment) refuses tap commands loudly.
#[test]
fn tap_without_segment_fails() {
    let server = TestServer::spawn();

    server.send("/bus_tap", vec![OscType::Int(0), OscType::Int(1)]);
    let fail = server.recv_until("/fail");
    assert_eq!(fail.args[0], OscType::String("/bus_tap".into()));

    server.send(
        "/bus_tapStream",
        vec![OscType::Int(50), OscType::Int(512), OscType::Int(0)],
    );
    let fail = server.recv_until("/fail");
    assert_eq!(fail.args[0], OscType::String("/bus_tapStream".into()));

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
        "/buffer_allocRead",
        vec![
            OscType::Int(0),
            OscType::String(path.to_str().unwrap().into()),
        ],
    );
    // Wait for the async load to install the buffer.
    server.recv_until("/done");

    // A range read, asking for more than the buffer holds: count clamps to 6.
    server.send(
        "/buffer_getRange",
        vec![OscType::Int(0), OscType::Int(0), OscType::Int(100)],
    );
    let reply = server.recv_until("/buffer_getRange.reply");
    assert_eq!(reply.args[0], OscType::Int(0)); // bufnum
    assert_eq!(reply.args[1], OscType::Int(0)); // start
    let got = range_samples(&reply.args[2]); // the blob's length is the count
    assert_eq!(got.len(), 6, "clamped to what the buffer holds");
    assert_eq!(got, samples);

    // A mid-range slice.
    server.send(
        "/buffer_getRange",
        vec![OscType::Int(0), OscType::Int(2), OscType::Int(3)],
    );
    let reply = server.recv_until("/buffer_getRange.reply");
    assert_eq!(reply.args[1], OscType::Int(2));
    // args = [bufnum, start, blob(samples[2..5])]
    let slice = range_samples(&reply.args[2]);
    assert_eq!(slice, vec![0.2, 0.3, 0.4]);

    // Indexed reads; an out-of-range index reads as 0.0.
    server.send(
        "/buffer_get",
        vec![OscType::Int(0), OscType::Int(3), OscType::Int(99)],
    );
    let reply = server.recv_until("/buffer_get.reply");
    assert_eq!(reply.args[0], OscType::Int(0));
    assert_eq!(reply.args[1], OscType::Int(3));
    assert_eq!(reply.args[2], OscType::Float(0.3));
    assert_eq!(reply.args[3], OscType::Int(99));
    assert_eq!(reply.args[4], OscType::Float(0.0));

    // An unallocated buffer yields an empty range (count 0), not an error.
    server.send(
        "/buffer_getRange",
        vec![OscType::Int(7), OscType::Int(0), OscType::Int(4)],
    );
    let reply = server.recv_until("/buffer_getRange.reply");
    assert_eq!(reply.args[0], OscType::Int(7));
    assert!(
        range_samples(&reply.args[2]).is_empty(),
        "empty blob, not an error"
    );

    let _ = std::fs::remove_file(&path);
    server.quit();
}

#[test]
fn b_gen_fills_a_wavetable_then_reads_it_back() {
    use clausters::dsp::wavetable::wt_interp;
    let server = TestServer::spawn();

    // A 256-sample buffer = a 128-point wavetable.
    server.send("/buffer_alloc", vec![OscType::Int(0), OscType::Int(256)]);
    server.recv_until("/done");

    // Fill it with a single sine partial, wavetable format, normalized+cleared.
    server.send(
        "/buffer_gen",
        vec![
            OscType::Int(0),
            OscType::String("sine1".into()),
            OscType::Int(1 + 2 + 4),
            OscType::Float(1.0),
        ],
    );
    let done = server.recv_until("/done");
    assert_eq!(done.args[0], OscType::String("/buffer_gen".into()));
    assert_eq!(done.args[1], OscType::Int(0));

    // Read the whole table back and confirm it reconstructs the sine.
    server.send(
        "/buffer_getRange",
        vec![OscType::Int(0), OscType::Int(0), OscType::Int(256)],
    );
    let reply = server.recv_until("/buffer_getRange.reply");
    let table = range_samples(&reply.args[2]);
    assert_eq!(table.len(), 256);
    let points = table.len() / 2;
    // Read through the same door the oscillators do: `wt_interp` takes a
    // buffer's own cells, so the wire's samples go back into a buffer first.
    let len = table.len();
    let table = clausters::dsp::buffer::Buffer::new(table, 1, len, 48_000.0);
    for k in 0..points {
        let expect = (std::f32::consts::TAU * k as f32 / points as f32).sin();
        assert!(
            (wt_interp(table.cells(), k, 0.0) - expect).abs() < 1e-3,
            "point {k}"
        );
    }

    // An unknown generator and an unallocated target both /fail.
    server.send(
        "/buffer_gen",
        vec![
            OscType::Int(0),
            OscType::String("bogus".into()),
            OscType::Int(0),
        ],
    );
    assert_eq!(
        server.recv_until("/fail").args[0],
        OscType::String("/buffer_gen".into())
    );
    server.send(
        "/buffer_gen",
        vec![
            OscType::Int(9),
            OscType::String("sine1".into()),
            OscType::Int(7),
            OscType::Float(1.0),
        ],
    );
    assert_eq!(
        server.recv_until("/fail").args[0],
        OscType::String("/buffer_gen".into())
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
        "/buffer_allocRead",
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
        "/buffer_export",
        vec![
            OscType::Int(0),
            OscType::String(out.to_str().unwrap().into()),
        ],
    );
    let done = server.recv_until("/done");
    assert_eq!(done.args[0], OscType::String("/buffer_export".into()));
    assert_eq!(done.args[1], OscType::Int(0));

    // The file is exactly the samples as little-endian f32 (what the GUI host maps).
    let bytes = std::fs::read(&out).unwrap();
    let got: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    assert_eq!(got, samples);

    // Exporting an unallocated buffer fails (and writes no file).
    server.send(
        "/buffer_export",
        vec![
            OscType::Int(7),
            OscType::String(out.to_str().unwrap().into()),
        ],
    );
    assert_eq!(
        server.recv_until("/fail").args[0],
        OscType::String("/buffer_export".into())
    );

    let _ = std::fs::remove_file(&wav);
    let _ = std::fs::remove_file(&out);
    server.quit();
}

/// A peer that edits shared samples writes into the cells and sends nothing,
/// so `/buffer_touch` is how every *other* client learns the span changed.
#[test]
fn a_touched_span_reaches_the_other_clients_and_not_the_writer() {
    let server = TestServer::spawn();
    server.send("/server_notify", vec![OscType::Int(1)]);
    server.recv_until("/done");
    server.send(
        "/buffer_alloc",
        vec![OscType::Int(0), OscType::Int(64), OscType::Int(1)],
    );
    server.recv_until("/done");

    // Another client -- standing in for a local peer holding the mapping --
    // says it wrote 16 frames of channel 0 from frame 8.
    let peer = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
    let touch = encoder::encode(&OscPacket::Message(OscMessage {
        addr: "/buffer_touch".into(),
        args: vec![
            OscType::Int(0),
            OscType::Int(0),
            OscType::Int(8),
            OscType::Int(16),
        ],
    }))
    .unwrap();
    peer.send_to(&touch, server.addr).unwrap();

    let msg = server.recv_until("/buffer_touched");
    assert_eq!(
        msg.args,
        vec![
            OscType::Int(0),
            OscType::Int(0),
            OscType::Int(8),
            OscType::Int(16)
        ]
    );

    // The writer is not told what it just did: a touch from the registered
    // client itself comes back as nothing, and the next reply is the one it
    // asked for.
    server.send(
        "/buffer_touch",
        vec![
            OscType::Int(0),
            OscType::Int(0),
            OscType::Int(0),
            OscType::Int(4),
        ],
    );
    server.send("/server_status", vec![]);
    assert_eq!(server.recv().addr, "/server_status.reply");

    server.quit();
}

#[test]
fn notify_clients_receive_n_go_and_n_end() {
    let mut server = TestServer::spawn();
    server.send("/server_notify", vec![OscType::Int(1)]);
    assert_eq!(server.recv_until("/done").args[1], OscType::Int(1));

    server.send(
        "/synth_new",
        vec![
            OscType::String("default".into()),
            OscType::Int(1000),
            OscType::Int(1),
            OscType::Int(0),
            OscType::String("amp".into()),
            OscType::Float(0.0),
        ],
    );
    let go = server.tick_until("/node_start");
    assert_eq!(go.args[0], OscType::Int(1000));
    assert_eq!(go.args[1], OscType::Int(0)); // parent: root group
    assert_eq!(go.args[4], OscType::Int(0)); // not a group

    server.send("/node_free", vec![OscType::Int(1000)]);
    let end = server.tick_until("/node_end");
    assert_eq!(end.args[0], OscType::Int(1000));
    assert_eq!(end.args[4], OscType::Int(0));

    server.quit();
}

/// **A node notification does not wait for the idle tick.**
///
/// `/node_start` and `/node_end` are posted by the audio thread and drained
/// where every other queue is, on the way out of the command loop's `recv` —
/// so with nothing else talking to the server they used to wait for the 100 ms
/// `GC_INTERVAL`, measured at 103-112 ms over a quiet connection. The audio
/// thread cannot poke the loop's waker (it may not send, lock or allocate), so
/// a `/server_notify` client shortens the tick instead, exactly as a stream
/// subscription does.
///
/// The other notify tests hide this: `tick_until` nudges the server with
/// `/server_status` between engine blocks, and every nudge is a packet that
/// wakes the loop. This one sends the command, runs the engine, and then does
/// nothing but wait — which is what a client watching the node tree does.
#[test]
fn a_node_notification_is_reported_without_waiting_for_the_idle_tick() {
    let mut server = TestServer::spawn();
    server.send("/server_notify", vec![OscType::Int(1)]);
    assert_eq!(server.recv_until("/done").args[1], OscType::Int(1));

    server.send(
        "/synth_new",
        vec![
            OscType::String("default".into()),
            OscType::Int(1000),
            OscType::Int(1),
            OscType::Int(0),
            OscType::String("amp".into()),
            OscType::Float(0.0),
        ],
    );
    // The engine applies the command and posts the event; the test thread is
    // the audio thread here, so this is where that happens. Ticking is not
    // traffic — **nothing below sends the server anything** while a
    // notification is outstanding, which is the whole point: it has to find
    // its own way out.
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    let mut await_event = |server: &mut TestServer, addr: &str| -> Duration {
        let start = std::time::Instant::now();
        loop {
            server.engine.process_block(&mut out);
            // A short wait, so the engine keeps being ticked while the command
            // loop decides to look. How long the *command* takes to reach the
            // engine is not what is measured — the warm-up below is there so
            // that trip is not on the clock.
            if server
                .try_recv_within(addr, Duration::from_millis(2))
                .is_some()
            {
                return start.elapsed();
            }
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "no {addr} in two seconds"
            );
        }
    };

    // Warm-up, untimed: this one pays for whatever the first packet, the first
    // block and the scheduler cost.
    await_event(&mut server, "/node_start");

    // And the timed leg, on a server that is now demonstrably awake and a
    // path that is now demonstrably warm.
    server.send("/node_free", vec![OscType::Int(1000)]);
    let elapsed = await_event(&mut server, "/node_end");
    assert!(
        elapsed < Duration::from_millis(50),
        "the notification waited for the idle tick: {elapsed:?}"
    );

    server.quit();
}

#[test]
fn notify_register_and_unregister() {
    let server = TestServer::spawn();

    server.send("/server_notify", vec![OscType::Int(1)]);
    let reply = server.recv();
    assert_eq!(reply.addr, "/done");
    assert_eq!(reply.args[0], OscType::String("/server_notify".into()));
    assert_eq!(reply.args[1], OscType::Int(1)); // first client gets ID 1

    // registering twice keeps the same ID
    server.send("/server_notify", vec![OscType::Int(1)]);
    assert_eq!(server.recv().args[1], OscType::Int(1));

    server.send("/server_notify", vec![OscType::Int(0)]);
    assert_eq!(server.recv().addr, "/done");

    server.quit();
}

#[test]
fn notify_bad_argument_fails() {
    let server = TestServer::spawn();
    server.send("/server_notify", vec![OscType::String("yes".into())]);
    assert_eq!(server.recv().addr, "/fail");
    server.quit();
}

// --- S9: side-effect UGens (SendTrig/SendReply/Poll), no Out required ---

/// A def whose only UGen is `SendTrig` (no `Out`) compiles, runs, and replies
/// `/node_trigger nodeID id value` to a `/server_notify` client when its trigger control fires.
#[test]
fn send_trig_replies_tr_and_needs_no_out() {
    let mut server = TestServer::spawn();
    server.send("/server_notify", vec![OscType::Int(1)]);
    assert_eq!(server.recv_until("/done").args[1], OscType::Int(1));

    // Output-less def: a trigger control feeds SendTrig(in, id=7, value=0.5).
    let json = r#"{
        "name": "trigtest",
        "controls": [{"name": "t", "rate": "tr", "default": 0.0}],
        "ugens": [
            {"kind": "SendTrig", "inputs": [{"control": 0}, {"const": 7.0}, {"const": 0.5}]}
        ]
    }"#;
    server.send(
        "/def_send",
        vec![
            OscType::String("synth".into()),
            OscType::Blob(json.as_bytes().to_vec()),
        ],
    );
    assert_eq!(
        server.recv_until("/done").args[0],
        OscType::String("/def_send".into())
    );

    server.send(
        "/synth_new",
        vec![
            OscType::String("trigtest".into()),
            OscType::Int(1000),
            OscType::Int(1),
            OscType::Int(0),
        ],
    );
    server.wait_for_synth_count(1);

    // Fire the trigger control: it holds 1 for one block (rising edge).
    server.send(
        "/node_set",
        vec![
            OscType::Int(1000),
            OscType::String("t".into()),
            OscType::Float(1.0),
        ],
    );
    let tr = server.tick_until("/node_trigger");
    assert_eq!(tr.args[0], OscType::Int(1000)); // node id
    assert_eq!(tr.args[1], OscType::Int(7)); // trigger id
    assert_eq!(tr.args[2], OscType::Float(0.5)); // value

    server.send("/node_free", vec![OscType::Int(1000)]);
    server.wait_for_synth_count(0);
    server.quit();
}

/// `SendReply` replies at a custom OSC address with `nodeID replyID value…`.
#[test]
fn send_reply_replies_at_custom_address() {
    let mut server = TestServer::spawn();
    server.send("/server_notify", vec![OscType::Int(1)]);
    assert_eq!(server.recv_until("/done").args[1], OscType::Int(1));

    let json = r#"{
        "name": "replytest",
        "controls": [{"name": "t", "rate": "tr", "default": 0.0}],
        "ugens": [
            {"kind": "SendReply", "label": "/custom",
             "inputs": [{"control": 0}, {"const": 42.0}, {"const": 1.5}, {"const": 2.5}]}
        ]
    }"#;
    server.send(
        "/def_send",
        vec![
            OscType::String("synth".into()),
            OscType::Blob(json.as_bytes().to_vec()),
        ],
    );
    assert_eq!(
        server.recv_until("/done").args[0],
        OscType::String("/def_send".into())
    );

    server.send(
        "/synth_new",
        vec![
            OscType::String("replytest".into()),
            OscType::Int(1001),
            OscType::Int(1),
            OscType::Int(0),
        ],
    );
    server.wait_for_synth_count(1);

    server.send(
        "/node_set",
        vec![
            OscType::Int(1001),
            OscType::String("t".into()),
            OscType::Float(1.0),
        ],
    );
    let reply = server.tick_until("/custom");
    assert_eq!(reply.args[0], OscType::Int(1001)); // node id
    assert_eq!(reply.args[1], OscType::Int(42)); // reply id
    assert_eq!(reply.args[2], OscType::Float(1.5));
    assert_eq!(reply.args[3], OscType::Float(2.5));

    server.send("/node_free", vec![OscType::Int(1001)]);
    server.wait_for_synth_count(0);
    server.quit();
}

/// `Poll` with a non-negative trigid also emits `/node_trigger nodeID trigid value`.
#[test]
fn poll_with_trigid_replies_tr() {
    let mut server = TestServer::spawn();
    server.send("/server_notify", vec![OscType::Int(1)]);
    assert_eq!(server.recv_until("/done").args[1], OscType::Int(1));

    // Poll(trig=t, in=0.25, trigid=3), labelled; passes `in` through (no Out).
    let json = r#"{
        "name": "polltest",
        "controls": [{"name": "t", "rate": "tr", "default": 0.0}],
        "ugens": [
            {"kind": "Poll", "label": "watch",
             "inputs": [{"control": 0}, {"const": 0.25}, {"const": 3.0}]}
        ]
    }"#;
    server.send(
        "/def_send",
        vec![
            OscType::String("synth".into()),
            OscType::Blob(json.as_bytes().to_vec()),
        ],
    );
    assert_eq!(
        server.recv_until("/done").args[0],
        OscType::String("/def_send".into())
    );

    server.send(
        "/synth_new",
        vec![
            OscType::String("polltest".into()),
            OscType::Int(1002),
            OscType::Int(1),
            OscType::Int(0),
        ],
    );
    server.wait_for_synth_count(1);

    server.send(
        "/node_set",
        vec![
            OscType::Int(1002),
            OscType::String("t".into()),
            OscType::Float(1.0),
        ],
    );
    let tr = server.tick_until("/node_trigger");
    assert_eq!(tr.args[0], OscType::Int(1002));
    assert_eq!(tr.args[1], OscType::Int(3)); // trigid
    assert_eq!(tr.args[2], OscType::Float(0.25)); // polled value

    server.send("/node_free", vec![OscType::Int(1002)]);
    server.wait_for_synth_count(0);
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
            addr: "/server_status".into(),
            args: vec![],
        })],
    });
    let bytes = encoder::encode(&bundle).unwrap();
    server.client.send_to(&bytes, server.addr).unwrap();
    assert_eq!(server.recv().addr, "/server_status.reply");
    server.quit();
}

/// M8/M21: `/clock_query` exposes the engine's sample counter, the actual sample
/// rate and the server's OSC/NTP time captured with the counter — the anchor a
/// client needs to place its clock on the server's sample axis.
#[test]
fn clock_reports_the_engine_sample_counter() {
    let mut server = TestServer::spawn();

    server.send("/clock_query", vec![]);
    let reply = server.recv_until("/clock_query.reply");
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
    server.send("/clock_query", vec![]);
    let reply = server.recv_until("/clock_query.reply");
    assert_eq!(reply.args[0], OscType::Long(10 * BLOCK_SIZE as i64));
    server.quit();
}

/// M22: `/transport_set` is the shared beat grid for phase alignment — a query
/// reports "undefined" until a client sets it, then echoes it back; bad args
/// fail and leave the previous grid intact.
#[test]
fn transport_query_and_set() {
    let server = TestServer::spawn();

    // Unset: defined flag 0, zeros. Reply is (origin, tempo, defined, playing,
    // position, group, transportSample, positionSample, loopStart, loopEnd) --
    // the grid, the rolling state, the governed group with the transport
    // clock, and where the piece is with the loop it wraps in. Everything past
    // the fifth field is appended, so a client reading the original five still
    // works.
    server.send("/transport_query", vec![]);
    let reply = server.recv_until("/transport_query.reply");
    assert_eq!(
        reply.args,
        vec![
            OscType::Long(0),
            OscType::Double(0.0),
            OscType::Int(0),
            OscType::Int(0),
            OscType::Double(0.0),
            OscType::Int(-1),
            OscType::Long(0),
            OscType::Long(0),
            OscType::Long(0),
            OscType::Long(0),
        ]
    );

    // Set origin sample + tempo; replies /done.
    server.send(
        "/transport_set",
        vec![OscType::Long(96_000), OscType::Double(2.0)],
    );
    assert_eq!(
        server.recv_until("/done").args[0],
        OscType::String("/transport_set".into())
    );

    // Query now reports the grid with defined 1, stopped at position 0.
    server.send("/transport_query", vec![]);
    let reply = server.recv_until("/transport_query.reply");
    assert_eq!(
        reply.args,
        vec![
            OscType::Long(96_000),
            OscType::Double(2.0),
            OscType::Int(1),
            OscType::Int(0),
            OscType::Double(0.0),
            OscType::Int(-1),
            OscType::Long(0),
            OscType::Long(0),
            OscType::Long(0),
            OscType::Long(0),
        ]
    );

    // Bad tempo fails and does not clobber the stored grid.
    server.send(
        "/transport_set",
        vec![OscType::Long(0), OscType::Double(0.0)],
    );
    assert_eq!(
        server.recv_until("/fail").args[0],
        OscType::String("/transport_set".into())
    );
    server.send("/transport_query", vec![]);
    assert_eq!(
        server.recv_until("/transport_query.reply").args[2],
        OscType::Int(1)
    );

    server.quit();
}

/// The DAW-style rolling state: play / stop / locate update `playing` and
/// `position`, reply `/done`, and need a grid defined first.
#[test]
fn transport_play_stop_locate() {
    let server = TestServer::spawn();

    // Only the commands that speak **beats** need a grid. A bare play needs
    // none -- the transport exists before any tempo does.
    server.send("/transport_play", vec![]);
    assert_eq!(
        server.recv_until("/done").args[0],
        OscType::String("/transport_play".into())
    );
    server.send("/transport_stop", vec![]);
    server.recv_until("/done");
    // A play *from a beat* does, and so does a locate by beat.
    server.send("/transport_play", vec![OscType::Double(8.0)]);
    assert_eq!(
        server.recv_until("/fail").args[0],
        OscType::String("/transport_play".into())
    );
    server.send("/transport_locate", vec![OscType::Double(8.0)]);
    assert_eq!(
        server.recv_until("/fail").args[0],
        OscType::String("/transport_locate".into())
    );

    server.send(
        "/transport_set",
        vec![OscType::Long(0), OscType::Double(2.0)],
    );
    server.recv_until("/done");

    // Play from beat 8: playing=1, position=8.
    server.send("/transport_play", vec![OscType::Double(8.0)]);
    server.recv_until("/done");
    server.send("/transport_query", vec![]);
    let reply = server.recv_until("/transport_query.reply");
    assert_eq!(reply.args[3], OscType::Int(1)); // playing
    assert_eq!(reply.args[4], OscType::Double(8.0)); // position

    // Locate to 16 while playing: position moves, playing unchanged.
    server.send("/transport_locate", vec![OscType::Double(16.0)]);
    server.recv_until("/done");
    server.send("/transport_query", vec![]);
    let reply = server.recv_until("/transport_query.reply");
    assert_eq!(reply.args[3], OscType::Int(1));
    assert_eq!(reply.args[4], OscType::Double(16.0));

    // Stop: playing=0, position holds.
    server.send("/transport_stop", vec![]);
    server.recv_until("/done");
    server.send("/transport_query", vec![]);
    let reply = server.recv_until("/transport_query.reply");
    assert_eq!(reply.args[3], OscType::Int(0));
    assert_eq!(reply.args[4], OscType::Double(16.0));

    server.quit();
}

/// A `/server_notify` client is pushed the new grid as a `/transport_query.reply` whenever
/// the transport is set, so its responders re-align without polling (M22
/// push-on-change paired with client responders).
#[test]
fn transport_pushes_on_change_to_notify_clients() {
    let server = TestServer::spawn();

    server.send("/server_notify", vec![OscType::Int(1)]);
    assert_eq!(server.recv_until("/done").args[1], OscType::Int(1));

    // Setting the transport replies /done to the setter and pushes the grid.
    server.send(
        "/transport_set",
        vec![OscType::Long(48_000), OscType::Double(2.0)],
    );
    let push = server.recv_until("/transport_query.reply");
    assert_eq!(
        push.args,
        vec![
            OscType::Long(48_000),
            OscType::Double(2.0),
            OscType::Int(1),
            OscType::Int(0),
            OscType::Double(0.0),
            OscType::Int(-1),
            OscType::Long(0),
            OscType::Long(0),
            OscType::Long(0),
            OscType::Long(0),
        ]
    );

    // A play also pushes the rolling state to the /server_notify client.
    server.send("/transport_play", vec![OscType::Double(4.0)]);
    let push = server.recv_until("/transport_query.reply");
    assert_eq!(push.args[3], OscType::Int(1));
    assert_eq!(push.args[4], OscType::Double(4.0));

    server.quit();
}

/// The sample-addressed half of the transport: an editor seeks by frame, and
/// the beat position follows so the two spellings never disagree.
#[test]
fn transport_locate_sample_moves_the_position_and_its_beat_reading() {
    let mut server = TestServer::spawn();

    // Needs **no** grid: a frame is a frame, and an audio editor has no tempo
    // to declare. The beat reading stays 0 while there is nothing to read it
    // against.
    server.send("/transport_locateSample", vec![OscType::Long(4_800)]);
    assert_eq!(
        server.recv_until("/done").args[0],
        OscType::String("/transport_locateSample".into())
    );
    server.send("/transport_query", vec![]);
    let reply = server.recv_until("/transport_query.reply");
    assert_eq!(reply.args[2], OscType::Int(0), "still no grid");
    assert_eq!(reply.args[4], OscType::Double(0.0), "and no beat to report");

    // Tempo is in beats per **second** (the grid is `origin + b*rate/tempo`,
    // sclang's TempoClock convention), so 2.0 makes one beat half a second.
    server.send(
        "/transport_set",
        vec![OscType::Long(0), OscType::Double(2.0)],
    );
    server.recv_until("/done");

    server.send("/transport_locateSample", vec![OscType::Long(24_000)]);
    assert_eq!(
        server.recv_until("/done").args[0],
        OscType::String("/transport_locateSample".into())
    );

    // The position is the engine's, published once a block, so the query
    // answers it as of the last completed one -- the same rule `/buffer_query`
    // states for the buffer mirror.
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    server.engine.process_block(&mut out);
    server.send("/transport_query", vec![]);
    let reply = server.recv_until("/transport_query.reply");
    assert_eq!(
        reply.args[7],
        OscType::Long(24_000),
        "the piece is where it was sent, in samples"
    );
    let OscType::Double(beats) = reply.args[4] else {
        panic!("the beat position is a double")
    };
    assert!(
        (beats - 1.0).abs() < 1e-9,
        "24000 samples at 2 beats/s and 48 kHz is beat 1, got {beats}"
    );

    server.quit();
}

/// `/transport_loop`: two arguments set the span, none clears it, and a span
/// that is not a span fails rather than being ignored.
#[test]
fn transport_loop_sets_and_clears_a_span() {
    let server = TestServer::spawn();

    // No grid: a loop is a span of samples and knows nothing about beats.
    server.send(
        "/transport_loop",
        vec![OscType::Long(1_000), OscType::Long(5_000)],
    );
    assert_eq!(
        server.recv_until("/done").args[0],
        OscType::String("/transport_loop".into())
    );
    server.send("/transport_query", vec![]);
    let reply = server.recv_until("/transport_query.reply");
    assert_eq!(reply.args[8], OscType::Long(1_000));
    assert_eq!(reply.args[9], OscType::Long(5_000));

    // An inverted span is always a mistake: it fails, and the live loop stands.
    server.send(
        "/transport_loop",
        vec![OscType::Long(5_000), OscType::Long(1_000)],
    );
    assert_eq!(
        server.recv_until("/fail").args[0],
        OscType::String("/transport_loop".into())
    );
    server.send("/transport_query", vec![]);
    assert_eq!(
        server.recv_until("/transport_query.reply").args[8],
        OscType::Long(1_000)
    );

    // No arguments turns looping off.
    server.send("/transport_loop", vec![]);
    server.recv_until("/done");
    server.send("/transport_query", vec![]);
    let reply = server.recv_until("/transport_query.reply");
    assert_eq!(reply.args[8], OscType::Long(0));
    assert_eq!(reply.args[9], OscType::Long(0));

    server.quit();
}

/// The whole editor path with **no beat grid at all**: bind a group, seek by
/// frame, loop a span, play and stop. An audio editor has no tempo to declare,
/// and asking it to invent one so the transport will answer was asking it to
/// write down a number nobody reads.
#[test]
fn the_transport_drives_a_piece_in_samples_with_no_grid() {
    let mut server = TestServer::spawn();

    server.send(
        "/group_new",
        vec![OscType::Int(100), OscType::Int(0), OscType::Int(0)],
    );
    server.wait_for_group_count(2); // the root group plus this one

    for (addr, args) in [
        ("/transport_group", vec![OscType::Int(100)]),
        ("/transport_locateSample", vec![OscType::Long(24_000)]),
        (
            "/transport_loop",
            vec![OscType::Long(0), OscType::Long(96_000)],
        ),
        ("/transport_play", vec![]),
        ("/transport_stop", vec![]),
    ] {
        server.send(addr, args);
        assert_eq!(
            server.recv_until("/done").args[0],
            OscType::String(addr.into()),
            "{addr} should not need a grid"
        );
    }

    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    server.engine.process_block(&mut out);
    server.send("/transport_query", vec![]);
    let reply = server.recv_until("/transport_query.reply");
    assert_eq!(reply.args[2], OscType::Int(0), "no grid was ever defined");
    assert_eq!(
        reply.args[5],
        OscType::Int(100),
        "and yet a group is governed"
    );
    assert_eq!(
        reply.args[7],
        OscType::Long(24_000),
        "and the piece is placed"
    );
    assert_eq!(reply.args[9], OscType::Long(96_000), "and looping");

    server.quit();
}

/// M8: `/sched_at` argument validation and per-message translation failures.
#[test]
fn sched_rejects_bad_arguments() {
    let server = TestServer::spawn();
    let s_new_blob = || {
        encoder::encode(&OscPacket::Message(OscMessage {
            addr: "/synth_new".into(),
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
    server.send("/sched_at", vec![]);
    assert_eq!(
        server.recv_until("/fail").args[0],
        OscType::String("/sched_at".into())
    );
    // Target without a packet blob.
    server.send("/sched_at", vec![OscType::Long(100)]);
    assert_eq!(
        server.recv_until("/fail").args[0],
        OscType::String("/sched_at".into())
    );
    // Negative target.
    server.send(
        "/sched_at",
        vec![OscType::Long(-1), OscType::Blob(s_new_blob())],
    );
    assert_eq!(
        server.recv_until("/fail").args[0],
        OscType::String("/sched_at".into())
    );
    // Garbage blob.
    server.send(
        "/sched_at",
        vec![OscType::Long(100), OscType::Blob(vec![1, 2, 3, 4])],
    );
    assert_eq!(
        server.recv_until("/fail").args[0],
        OscType::String("/sched_at".into())
    );
    // A query is not schedulable: the /fail names the offending message.
    let status_blob = encoder::encode(&OscPacket::Message(OscMessage {
        addr: "/server_status".into(),
        args: vec![],
    }))
    .unwrap();
    server.send(
        "/sched_at",
        vec![OscType::Long(100), OscType::Blob(status_blob)],
    );
    assert_eq!(
        server.recv_until("/fail").args[0],
        OscType::String("/server_status".into())
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
        // A far-future NTP tag that must be ignored: /sched_at is the clock.
        timetag: OscTime {
            seconds: u32::MAX,
            fractional: 0,
        },
        content: vec![OscPacket::Message(OscMessage {
            addr: "/synth_new".into(),
            args: vec![
                OscType::String("default".into()),
                OscType::Int(1000),
                OscType::Int(1),
                OscType::Int(0),
            ],
        })],
    });
    server.send(
        "/sched_at",
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
    client.send("/server_status", vec![]);
    assert_eq!(
        client.recv_until("/server_status.reply").addr,
        "/server_status.reply"
    );

    // A SynthDef sent over TCP compiles and the async /done comes back framed
    // on the same connection.
    let spec = br#"{"name":"tcp_def","controls":[],"ugens":[{"kind":"WhiteNoise","inputs":[]},{"kind":"Out","inputs":[{"const":0.0},{"ugen":0}]}]}"#;
    client.send(
        "/def_send",
        vec![
            OscType::String("synth".into()),
            OscType::Blob(spec.to_vec()),
        ],
    );
    assert_eq!(
        client.recv_until("/done").args[0],
        OscType::String("/def_send".into())
    );
}

#[test]
fn tcp_replies_route_to_the_originating_connection() {
    let (tcp_addr, _join, _engine) = spawn_tcp_server();
    let mut a = TcpClient::connect(tcp_addr);
    let mut b = TcpClient::connect(tcp_addr);

    // Only `a` asks: only `a` must receive the reply (per-connection routing).
    a.send("/server_status", vec![]);
    assert_eq!(
        a.recv_until("/server_status.reply").addr,
        "/server_status.reply"
    );

    // `b` is still healthy on its own connection afterwards.
    b.send("/server_status", vec![]);
    assert_eq!(
        b.recv_until("/server_status.reply").addr,
        "/server_status.reply"
    );
}

/// M25: the stream transports carry frames well past the UDP datagram cap —
/// a ~200 KB `/buffer_gen env` request (10k breakpoints) goes in as one frame, and
/// the whole 40k-sample buffer comes back in one equally large `/buffer_getRange.reply`
/// reply to a single `/buffer_getRange`, no chunking either way.
#[test]
fn tcp_carries_frames_larger_than_a_datagram() {
    let (tcp_addr, _join, _engine) = spawn_tcp_server();
    let mut client = TcpClient::connect(tcp_addr);

    const N: usize = 40_000;
    const SEGS: usize = 10_000;
    client.send(
        "/buffer_alloc",
        vec![OscType::Int(0), OscType::Int(N as i32), OscType::Int(1)],
    );
    assert_eq!(
        client.recv_until("/done").args[0],
        OscType::String("/buffer_alloc".into())
    );

    // One /buffer_gen env frame with 10k linear segments stepping 0 -> SEGS in
    // equal times: 40k float args, far over the old 64 KiB ceiling.
    let mut args = vec![
        OscType::Int(0),
        OscType::String("env".into()),
        OscType::Float(0.0), // level0
    ];
    for i in 0..SEGS {
        args.extend([
            OscType::Float((i + 1) as f32), // level
            OscType::Float(1.0),            // time (relative)
            OscType::Float(1.0),            // shape: linear
            OscType::Float(0.0),            // curve
        ]);
    }
    client.send("/buffer_gen", args);
    assert_eq!(
        client.recv_until("/done").args[0],
        OscType::String("/buffer_gen".into())
    );

    // Read the whole buffer back in a single equally large reply.
    client.send(
        "/buffer_getRange",
        vec![OscType::Int(0), OscType::Int(0), OscType::Int(N as i32)],
    );
    let reply = client.recv_until("/buffer_getRange.reply");
    let samples = range_samples(&reply.args[2]);
    assert_eq!(samples.len(), N, "one reply frame carries it whole");
    // The ramp's tail sits at the last segment's level.
    let last = samples[N - 1];
    assert!((last - SEGS as f32).abs() < 2.0, "ramp tail, got {last}");
}

#[test]
fn sync_answers_synced_with_the_same_id() {
    let server = TestServer::spawn();
    // Nothing async outstanding: /server_sync.reply comes back immediately, echoing the id.
    server.send("/server_sync", vec![OscType::Int(42)]);
    let reply = server.recv_until("/server_sync.reply");
    assert_eq!(reply.args, vec![OscType::Int(42)]);
    server.quit();
}

#[test]
fn sync_waits_for_an_async_buffer_alloc() {
    let server = TestServer::spawn();
    // Queue an async buffer alloc (runs on the NRT thread), then the barrier.
    server.send(
        "/buffer_alloc",
        vec![OscType::Int(0), OscType::Int(64), OscType::Int(1)],
    );
    server.send("/server_sync", vec![OscType::Int(7)]);

    // The barrier must not answer before the alloc's /done lands.
    let mut saw_done = false;
    for _ in 0..100 {
        let msg = server.recv();
        if msg.addr == "/done" && msg.args.first() == Some(&OscType::String("/buffer_alloc".into()))
        {
            saw_done = true;
        }
        if msg.addr == "/server_sync.reply" {
            assert_eq!(msg.args, vec![OscType::Int(7)]);
            assert!(
                saw_done,
                "/server_sync.reply arrived before the buffer's /done"
            );
            server.quit();
            return;
        }
    }
    panic!("never received /server_sync.reply");
}

// ---- S6: OSC command-set completion ----

impl TestServer {
    /// The ordered synth child IDs of a group, read from `/group_queryTree` (each
    /// synth child is `Int(id), Int(-1), String(defName)`).
    fn group_child_ids(&self, group: i32) -> Vec<i32> {
        self.send(
            "/group_queryTree",
            vec![OscType::Int(group), OscType::Int(0)],
        );
        let reply = self.recv_until("/group_queryTree.reply");
        let mut ids = Vec::new();
        for pair in reply.args.windows(2) {
            if let [OscType::Int(id), OscType::Int(-1)] = pair
                && *id > 0
            {
                ids.push(*id);
            }
        }
        ids
    }

    /// Fresh "default" synth `id` at the tail of group `parent`.
    fn new_synth(&self, id: i32, parent: i32) {
        self.new_synth_named("default", id, parent);
    }

    /// Fresh synth of def `name`, id `id`, at the tail of group `parent`.
    fn new_synth_named(&self, name: &str, id: i32, parent: i32) {
        self.send(
            "/synth_new",
            vec![
                OscType::String(name.into()),
                OscType::Int(id),
                OscType::Int(1),
                OscType::Int(parent),
            ],
        );
    }

    /// Asserts the next reply to a just-sent command is *not* a `/fail` (a
    /// following `/server_status.reply` proves the command was accepted silently).
    fn assert_accepted(&self, cmd: &str) {
        self.send("/server_status", vec![]);
        let reply = self.recv();
        assert_eq!(
            reply.addr, "/server_status.reply",
            "{cmd} unexpectedly failed"
        );
    }
}

#[test]
fn n_setn_and_s_get_read_a_control_range() {
    let mut server = TestServer::spawn();
    server.new_synth(1000, 0);
    server.wait_for_synth_count(1);

    // Set both controls (freq@0, amp@1) as one range.
    server.send(
        "/node_setRange",
        vec![
            OscType::Int(1000),
            OscType::Int(0),
            OscType::Int(2),
            OscType::Float(550.0),
            OscType::Float(0.5),
        ],
    );
    server.assert_accepted("/node_setRange");

    // /synth_get by name echoes (control, value) pairs.
    server.send(
        "/synth_get",
        vec![OscType::Int(1000), OscType::String("freq".into())],
    );
    let reply = server.recv_until("/node_set");
    assert_eq!(
        reply.args,
        vec![OscType::Int(1000), OscType::Int(0), OscType::Float(550.0)]
    );

    // /synth_getRange returns a whole range as (control, numControls, val...).
    server.send(
        "/synth_getRange",
        vec![OscType::Int(1000), OscType::Int(0), OscType::Int(2)],
    );
    let reply = server.recv_until("/node_set");
    assert_eq!(
        reply.args,
        vec![
            OscType::Int(1000),
            OscType::Int(0),
            OscType::Int(2),
            OscType::Float(550.0),
            OscType::Float(0.5),
        ]
    );

    server.quit();
}

#[test]
fn n_fill_fills_a_control_range() {
    let mut server = TestServer::spawn();
    server.new_synth(1000, 0);
    server.wait_for_synth_count(1);

    server.send(
        "/node_fill",
        vec![
            OscType::Int(1000),
            OscType::Int(0),
            OscType::Int(2),
            OscType::Float(0.7),
        ],
    );
    server.assert_accepted("/node_fill");

    server.send(
        "/synth_getRange",
        vec![OscType::Int(1000), OscType::Int(0), OscType::Int(2)],
    );
    let reply = server.recv_until("/node_set");
    assert_eq!(reply.args[3], OscType::Float(0.7));
    assert_eq!(reply.args[4], OscType::Float(0.7));

    server.quit();
}

#[test]
fn n_mapn_is_accepted_and_rejects_unknown_node() {
    let mut server = TestServer::spawn();
    server.new_synth(1000, 0);
    server.wait_for_synth_count(1);

    // Map freq@0,amp@1 to control buses 3,4.
    server.send(
        "/node_mapRange",
        vec![
            OscType::Int(1000),
            OscType::Int(0),
            OscType::Int(3),
            OscType::Int(2),
        ],
    );
    server.assert_accepted("/node_mapRange");

    server.send(
        "/node_mapRange",
        vec![
            OscType::Int(4242),
            OscType::Int(0),
            OscType::Int(3),
            OscType::Int(2),
        ],
    );
    let reply = server.recv();
    assert_eq!(reply.addr, "/fail");
    assert_eq!(reply.args[0], OscType::String("/node_mapRange".into()));

    server.quit();
}

#[test]
fn g_head_g_tail_and_n_order_reorder_children() {
    let mut server = TestServer::spawn();
    server.send(
        "/group_new",
        vec![OscType::Int(1), OscType::Int(1), OscType::Int(0)],
    );
    server.wait_for_group_count(2);
    for id in [1001, 1002, 1003] {
        server.new_synth(id, 1);
    }
    server.wait_for_synth_count(3);
    assert_eq!(server.group_child_ids(1), vec![1001, 1002, 1003]);

    // /group_head moves a node to the front, /group_tail to the back.
    server.send("/group_head", vec![OscType::Int(1), OscType::Int(1003)]);
    assert_eq!(server.group_child_ids(1), vec![1003, 1001, 1002]);
    server.send("/group_tail", vec![OscType::Int(1), OscType::Int(1003)]);
    assert_eq!(server.group_child_ids(1), vec![1001, 1002, 1003]);

    // /node_order addAction 0 (head), keeping the listed order.
    server.send(
        "/node_order",
        vec![
            OscType::Int(0),
            OscType::Int(1),
            OscType::Int(1003),
            OscType::Int(1002),
        ],
    );
    assert_eq!(server.group_child_ids(1), vec![1003, 1002, 1001]);

    server.quit();
}

#[test]
fn g_head_rejects_auto_sorted_group() {
    let mut server = TestServer::spawn();
    server.send(
        "/group_new",
        vec![OscType::Int(1), OscType::Int(1), OscType::Int(0)],
    );
    server.wait_for_group_count(2);
    server.new_synth(1001, 1);
    server.wait_for_synth_count(1);
    // Turn on auto-sort: manual moves into the group must /fail.
    server.send("/group_sortMode", vec![OscType::Int(1), OscType::Int(1)]);

    server.send("/group_head", vec![OscType::Int(1), OscType::Int(1001)]);
    let reply = server.recv();
    assert_eq!(reply.addr, "/fail");
    assert_eq!(reply.args[0], OscType::String("/group_head".into()));

    server.quit();
}

#[test]
fn c_setn_c_getn_and_c_fill_roundtrip() {
    let server = TestServer::spawn();

    // Set a 3-bus range from bus 10.
    server.send(
        "/bus_setRange",
        vec![
            OscType::Int(10),
            OscType::Int(3),
            OscType::Float(0.1),
            OscType::Float(0.2),
            OscType::Float(0.3),
        ],
    );
    server.send("/bus_getRange", vec![OscType::Int(10), OscType::Int(3)]);
    let reply = server.recv_until("/bus_getRange.reply");
    assert_eq!(
        reply.args,
        vec![
            OscType::Int(10),
            OscType::Int(3),
            OscType::Float(0.1),
            OscType::Float(0.2),
            OscType::Float(0.3),
        ]
    );

    // Fill overwrites the whole range with one value.
    server.send(
        "/bus_fill",
        vec![OscType::Int(10), OscType::Int(3), OscType::Float(0.9)],
    );
    server.send("/bus_getRange", vec![OscType::Int(10), OscType::Int(3)]);
    let reply = server.recv_until("/bus_getRange.reply");
    assert_eq!(reply.args[2], OscType::Float(0.9));
    assert_eq!(reply.args[4], OscType::Float(0.9));

    server.quit();
}

#[test]
fn s_noid_acknowledges_and_rejects_unknown() {
    let mut server = TestServer::spawn();
    server.new_synth(1000, 0);
    server.wait_for_synth_count(1);

    server.send("/synth_forgetId", vec![OscType::Int(1000)]);
    let reply = server.recv_until("/done");
    assert_eq!(reply.args[0], OscType::String("/synth_forgetId".into()));

    server.send("/synth_forgetId", vec![OscType::Int(4242)]);
    let reply = server.recv();
    assert_eq!(reply.addr, "/fail");
    assert_eq!(reply.args[0], OscType::String("/synth_forgetId".into()));

    server.quit();
}

#[test]
fn b_close_acknowledges_live_buffer_and_rejects_missing() {
    let server = TestServer::spawn();
    server.send(
        "/buffer_alloc",
        vec![OscType::Int(0), OscType::Int(64), OscType::Int(1)],
    );
    server.recv_until("/done");

    server.send("/buffer_close", vec![OscType::Int(0)]);
    let reply = server.recv_until("/done");
    assert_eq!(reply.args[0], OscType::String("/buffer_close".into()));
    assert_eq!(reply.args[1], OscType::Int(0));

    server.send("/buffer_close", vec![OscType::Int(5)]);
    let reply = server.recv();
    assert_eq!(reply.addr, "/fail");
    assert_eq!(reply.args[0], OscType::String("/buffer_close".into()));

    server.quit();
}

#[test]
fn d_load_reads_a_synthdef_from_disk() {
    let mut server = TestServer::spawn();
    let json = r#"{
        "name": "loaded",
        "controls": [{"name": "freq", "default": 210.0}],
        "ugens": [
            {"kind": "Sine", "inputs": [{"control": 0}]},
            {"kind": "Mul",    "inputs": [{"ugen": 0}, {"const": 0.1}]},
            {"kind": "Out",    "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]
    }"#;
    let path = std::env::temp_dir().join(format!("clausters_d_load_{}.json", std::process::id()));
    std::fs::write(&path, json).unwrap();

    server.send(
        "/def_load",
        vec![OscType::String(path.to_string_lossy().into())],
    );
    let reply = server.recv_until("/done");
    assert_eq!(reply.args[0], OscType::String("/def_load".into()));

    // The loaded def is now instantiable.
    server.new_synth_named("loaded", 1000, 0);
    server.wait_for_synth_count(1);

    std::fs::remove_file(&path).ok();
    server.quit();
}

#[test]
fn d_load_missing_file_fails() {
    let server = TestServer::spawn();
    server.send(
        "/def_load",
        vec![OscType::String("/no/such/def.json".into())],
    );
    let reply = server.recv();
    assert_eq!(reply.addr, "/fail");
    assert_eq!(reply.args[0], OscType::String("/def_load".into()));
    server.quit();
}

#[test]
fn clear_sched_flushes_pending_bundles() {
    use clausters::rosc::OscBundle;

    let mut server = TestServer::spawn();
    // Schedule a synth ~15 blocks out, then flush before the clock reaches it.
    let bundle = OscPacket::Bundle(OscBundle {
        timetag: clausters::rosc::OscTime {
            seconds: 0,
            fractional: 0,
        },
        content: vec![OscPacket::Message(OscMessage {
            addr: "/synth_new".into(),
            args: vec![
                OscType::String("default".into()),
                OscType::Int(1000),
                OscType::Int(1),
                OscType::Int(0),
            ],
        })],
    });
    server.send(
        "/sched_at",
        vec![
            OscType::Long(BLOCK_SIZE as i64 * 15),
            OscType::Blob(encoder::encode(&bundle).unwrap()),
        ],
    );
    server.send("/sched_clear", vec![]);
    let reply = server.recv_until("/done");
    assert_eq!(reply.args[0], OscType::String("/sched_clear".into()));

    // Tick well past the target: the flushed bundle must never fire.
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    for _ in 0..40 {
        server.engine.process_block(&mut out);
    }
    server.send("/server_status", vec![]);
    assert_eq!(
        server.recv_until("/server_status.reply").args[1],
        OscType::Int(0),
        "a cleared bundle must not spawn its synth"
    );

    server.quit();
}

#[test]
fn error_mode_still_replies_fail() {
    let server = TestServer::spawn();
    // Silence console posting; the /fail OSC reply must still be sent.
    server.send("/server_errorMode", vec![OscType::Int(0)]);
    server.send("/node_set", vec![OscType::Int(4242), OscType::Float(1.0)]);
    let reply = server.recv();
    assert_eq!(reply.addr, "/fail");
    server.send("/server_errorMode", vec![OscType::Int(1)]);
    server.quit();
}

#[test]
fn cmd_ping_and_unknown_command() {
    let server = TestServer::spawn();
    server.send("/server_cmd", vec![OscType::String("ping".into())]);
    let reply = server.recv_until("/done");
    assert_eq!(reply.args[0], OscType::String("/server_cmd".into()));
    assert_eq!(reply.args[1], OscType::String("ping".into()));

    server.send("/server_cmd", vec![OscType::String("bogus".into())]);
    let reply = server.recv();
    assert_eq!(reply.addr, "/fail");
    assert_eq!(reply.args[0], OscType::String("/server_cmd".into()));

    server.quit();
}

#[test]
fn u_cmd_validates_target_and_index() {
    let mut server = TestServer::spawn();
    server.new_synth(1000, 0);
    server.wait_for_synth_count(1);

    // A valid /node_ugenCmd to an in-range UGen (default synth has 3 UGens) is
    // accepted silently — the default handler ignores it.
    server.send(
        "/node_ugenCmd",
        vec![
            OscType::Int(1000),
            OscType::Int(0),
            OscType::String("noop".into()),
            OscType::Float(1.0),
        ],
    );
    server.assert_accepted("/node_ugenCmd");

    // Out-of-range UGen index fails.
    server.send(
        "/node_ugenCmd",
        vec![
            OscType::Int(1000),
            OscType::Int(99),
            OscType::String("noop".into()),
        ],
    );
    let reply = server.recv();
    assert_eq!(reply.addr, "/fail");
    assert_eq!(reply.args[0], OscType::String("/node_ugenCmd".into()));

    // Unknown node fails.
    server.send(
        "/node_ugenCmd",
        vec![
            OscType::Int(4242),
            OscType::Int(0),
            OscType::String("noop".into()),
        ],
    );
    assert_eq!(server.recv().addr, "/fail");

    server.quit();
}

/// S7: `/server_query.reply` reports the boot-time pool capacities and I/O
/// channels so a client can size its own allocators from the server. The first
/// six fields stay stable; the S7 fields are appended.
#[test]
fn server_info_reports_configured_limits() {
    let limits = Limits {
        max_nodes: 512,
        max_buffers: 64,
        max_group_children: 32,
        max_ugen_inputs: 24,
    };
    let server = TestServer::spawn_with_limits(limits);
    server.send("/server_query", vec![]);
    let reply = server.recv_until("/server_query.reply");
    let ints: Vec<i32> = reply
        .args
        .iter()
        .map(|a| match a {
            OscType::Int(n) => *n,
            OscType::Double(_) => -1, // the two sample-rate fields
            other => panic!("unexpected arg {other:?}"),
        })
        .collect();
    // [audio_buses, control_buses, out_ch, block, sr, sr, in_ch, max_nodes,
    //  max_buffers, max_graph_children, max_ugen_inputs, taps, tap_frames]
    assert_eq!(ints[0], 128, "audio buses");
    assert_eq!(ints[2], 2, "output channels");
    assert_eq!(ints[6], 0, "no live input attached in this harness");
    assert_eq!(ints[7], 512, "max_nodes");
    assert_eq!(ints[8], 64, "max_buffers");
    assert_eq!(ints[9], 32, "max_graph_children");
    assert_eq!(ints[10], 24, "max_ugen_inputs");
    // No segment in this harness: the tap region reports empty.
    assert_eq!(ints[11], 0, "taps");
    assert_eq!(ints[12], 0, "tap_frames");
    // M25: the stream-transport frame ceiling, for clients to size bulk
    // requests from.
    assert_eq!(ints[13], 16 * 1024 * 1024, "max_frame");
    // The `/bus_stream` ceiling, per carrier: this client is a datagram one,
    // so it is the overview byte budget rather than the configured 4096 --
    // which is the whole reason the field is reported to the client instead of
    // being a constant it could have compiled in.
    assert!(
        (1..=4096).contains(&ints[14]),
        "max_stream_buses for a datagram client: {}",
        ints[14]
    );
    assert!(
        ints[14] > 128,
        "the old constant is not the ceiling any more"
    );
    server.quit();
}

/// S7: `--max-ugen-inputs` is enforced when a def is received; a def whose UGen
/// asks for more inputs than the configured limit is rejected with `/fail`.
#[test]
fn d_recv_rejects_over_max_ugen_inputs() {
    let limits = Limits {
        max_ugen_inputs: 2,
        ..Limits::default()
    };
    let server = TestServer::spawn_with_limits(limits);
    // Sum3 takes 3 inputs — over the limit of 2.
    let def = serde_json::json!({
        "name": "too_wide",
        "ugens": [
            {"kind": "Sum3", "inputs": [{"const": 1.0}, {"const": 2.0}, {"const": 3.0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    })
    .to_string();
    server.send(
        "/def_send",
        vec![OscType::String("synth".into()), OscType::String(def)],
    );
    assert_eq!(server.recv_until("/fail").addr, "/fail");
    server.quit();
}

// --- M30: the introspection verbs (/def_query, /ugen_query; /buffer_query's listing
//     form lives in tests/buffers.rs beside the other buffer coverage) ---

/// `/def_query` with no argument lists every loaded def with its control
/// surface, terminated by `/done`. The built-in "default" is always there, so
/// a fresh server already answers something.
#[test]
fn d_query_lists_loaded_defs_with_their_controls() {
    let server = TestServer::spawn();
    let def = serde_json::json!({
        "name": "introspected",
        "controls": [
            {"name": "freq", "default": 440.0},
            {"name": "gate", "default": 1.0, "rate": "tr"},
            {"name": "seed", "default": 7.0, "rate": "ir"}
        ],
        "ugens": [
            {"kind": "Sine", "inputs": [{"control": 0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    })
    .to_string();
    server.send(
        "/def_send",
        vec![OscType::String("synth".into()), OscType::String(def)],
    );
    server.recv_until("/done");

    server.send("/def_query", vec![]);
    let infos = server.recv_batch("/def_query.reply", "/def_query");
    let names: Vec<String> = infos
        .iter()
        .map(|m| match &m.args[0] {
            OscType::String(s) => s.clone(),
            other => panic!("expected a def name, got {other:?}"),
        })
        .collect();
    assert!(names.contains(&"default".to_string()), "{names:?}");
    assert!(names.contains(&"introspected".to_string()), "{names:?}");

    let one = infos
        .iter()
        .find(|m| m.args[0] == OscType::String("introspected".into()))
        .unwrap();
    assert_eq!(one.args[1], OscType::String("synth".into()));
    assert_eq!(one.args[2], OscType::Int(3), "three controls");
    // Then (name, default, type) per control, in declaration order.
    assert_eq!(one.args[3], OscType::String("freq".into()));
    assert_eq!(one.args[4], OscType::Float(440.0));
    assert_eq!(one.args[5], OscType::String("kr".into()));
    assert_eq!(one.args[6], OscType::String("gate".into()));
    assert_eq!(one.args[8], OscType::String("tr".into()));
    assert_eq!(one.args[9], OscType::String("seed".into()));
    assert_eq!(one.args[10], OscType::Float(7.0));
    assert_eq!(one.args[11], OscType::String("ir".into()));
    server.quit();
}

/// Named form: only the asked-for defs come back, and an unknown name reports
/// an empty family instead of failing the batch (the `/buffer_query` convention).
#[test]
fn d_query_details_named_defs_and_reports_unknown_as_empty() {
    let server = TestServer::spawn();
    server.send(
        "/def_query",
        vec![
            OscType::String("default".into()),
            OscType::String("nonexistent".into()),
        ],
    );
    let infos = server.recv_batch("/def_query.reply", "/def_query");
    assert_eq!(infos.len(), 2, "one reply per requested name");
    assert_eq!(infos[0].args[0], OscType::String("default".into()));
    assert_eq!(infos[0].args[1], OscType::String("synth".into()));
    assert_eq!(infos[1].args[0], OscType::String("nonexistent".into()));
    assert_eq!(infos[1].args[1], OscType::String(String::new()));
    assert_eq!(infos[1].args[2], OscType::Int(0));
    server.quit();
}

/// `/ugen_query <kind>` reports the descriptor a client palette needs: the named
/// inputs in wire order with their defaults, plus the rate rules.
#[test]
fn u_query_reports_a_ugen_signature() {
    let server = TestServer::spawn();
    server.send("/ugen_query", vec![OscType::String("Sine".into())]);
    let infos = server.recv_batch("/ugen_query.reply", "/ugen_query");
    assert_eq!(infos.len(), 1);
    let a = &infos[0].args;
    // name, arity, defaultRate, rates, exec, bus, needsPath, opFamily,
    // spectral, numInputs, then (name, default) per input.
    assert_eq!(a[0], OscType::String("Sine".into()));
    assert_eq!(a[1], OscType::Int(1), "arity");
    assert_eq!(a[2], OscType::String("ar".into()), "default rate");
    assert_eq!(a[3], OscType::String("kr,ar".into()), "allowed rates");
    assert_eq!(a[4], OscType::String("normal".into()));
    assert_eq!(a[5], OscType::String(String::new()), "no bus role");
    assert_eq!(a[6], OscType::Int(0), "needs no path");
    assert_eq!(a[9], OscType::Int(1), "one named input");
    assert_eq!(a[10], OscType::String("freq".into()));
    assert_eq!(a[11], OscType::Float(440.0));
    server.quit();
}

/// A variadic kind reports `-1` for its arity and names only its fixed head;
/// a bus-role kind reports the role the graph analysis reads.
#[test]
fn u_query_reports_variadic_arity_and_bus_roles() {
    let server = TestServer::spawn();
    server.send(
        "/ugen_query",
        vec![
            OscType::String("EnvGen".into()),
            OscType::String("Out".into()),
            OscType::String("NoSuchUGen".into()),
        ],
    );
    let infos = server.recv_batch("/ugen_query.reply", "/ugen_query");
    assert_eq!(infos.len(), 3);

    let env = &infos[0].args;
    assert_eq!(env[1], OscType::Int(-1), "variadic");
    assert_eq!(env[9], OscType::Int(5), "the five named head slots");
    assert_eq!(env[10], OscType::String("gate".into()));
    assert_eq!(env[11], OscType::Float(1.0));

    let out = &infos[1].args;
    assert_eq!(out[5], OscType::String("write".into()), "bus role");
    assert_eq!(out[10], OscType::String("bus".into()));
    assert_eq!(out[12], OscType::String("signal".into()));

    // Unknown kinds report empty rather than failing the batch.
    assert_eq!(infos[2].args[0], OscType::String("NoSuchUGen".into()));
    assert_eq!(infos[2].args[3], OscType::String(String::new()));
    assert_eq!(infos[2].args[9], OscType::Int(0));
    server.quit();
}

/// No argument returns the whole catalog — the palette's source. Every entry
/// must be well-formed (the arity/name agreement the registry unit test
/// guards, checked here across the wire).
#[test]
fn u_query_lists_the_whole_catalog() {
    let server = TestServer::spawn();
    server.send("/ugen_query", vec![]);
    let infos = server.recv_batch("/ugen_query.reply", "/ugen_query");
    assert!(infos.len() > 40, "got {} entries", infos.len());
    for m in &infos {
        let OscType::Int(num_inputs) = m.args[9] else {
            panic!("numInputs must be an int");
        };
        assert_eq!(
            m.args.len(),
            10 + 2 * num_inputs as usize,
            "{:?} declares {num_inputs} inputs but carries {} args",
            m.args[0],
            m.args.len()
        );
    }
    let names: Vec<&OscType> = infos.iter().map(|m| &m.args[0]).collect();
    assert!(names.contains(&&OscType::String("PlayBuf".into())));
    assert!(names.contains(&&OscType::String("FFT".into())));
    server.quit();
}

#[test]
fn a_group_is_born_named_and_its_death_says_which_one_died() {
    // The name travels with `/group_new`, and both node notifications carry
    // it: a client watching the tree learns which channel came up or went
    // away without a follow-up query — and for a death there is none to make.
    let mut server = TestServer::spawn();
    server.send("/server_notify", vec![OscType::Int(1)]);
    server.recv_until("/done");

    server.send(
        "/group_new",
        vec![
            OscType::Int(100),
            OscType::Int(0),
            OscType::Int(0),
            OscType::String("mixer".into()),
            // A second group in the same message, unnamed: the cursor reads
            // the optional label without losing the triples after it.
            OscType::Int(101),
            OscType::Int(0),
            OscType::Int(0),
        ],
    );
    let go = server.tick_until("/node_start");
    assert_eq!(go.args[0], OscType::Int(100));
    assert_eq!(go.args[4], OscType::Int(1)); // is a group
    assert_eq!(go.args[5], OscType::String("mixer".into()));

    let go = server.tick_until("/node_start");
    assert_eq!(go.args[0], OscType::Int(101));
    assert_eq!(go.args[5], OscType::String("".into()));

    server.send("/group_query", vec![OscType::String("/mixer".into())]);
    assert_eq!(
        server.recv_until("/group_query.reply").args[1],
        OscType::Int(100)
    );

    server.send("/node_free", vec![OscType::Int(100)]);
    let end = server.tick_until("/node_end");
    assert_eq!(end.args[0], OscType::Int(100));
    assert_eq!(end.args[5], OscType::String("mixer".into()));

    // And the name is free again the moment the group is gone.
    server.send(
        "/group_new",
        vec![
            OscType::Int(102),
            OscType::Int(0),
            OscType::Int(0),
            OscType::String("mixer".into()),
        ],
    );
    server.send("/group_query", vec![OscType::String("/mixer".into())]);
    assert_eq!(
        server.recv_until("/group_query.reply").args[1],
        OscType::Int(102)
    );
    server.quit();
}

#[test]
fn a_group_with_a_refused_name_is_not_created() {
    // The name is judged before the group exists, so a refused label refuses
    // the whole creation: a client that asked for a named group is never left
    // holding an anonymous one it did not ask for.
    let server = TestServer::spawn();
    server.send(
        "/group_new",
        vec![
            OscType::Int(100),
            OscType::Int(0),
            OscType::Int(0),
            OscType::String("mixer".into()),
        ],
    );
    server.send(
        "/group_new",
        vec![
            OscType::Int(101),
            OscType::Int(0),
            OscType::Int(0),
            OscType::String("mixer".into()), // taken by a sibling
        ],
    );
    assert_eq!(
        server.recv_until("/fail").args[0],
        OscType::String("/group_new".into())
    );
    server.send(
        "/group_new",
        vec![
            OscType::Int(102),
            OscType::Int(0),
            OscType::Int(0),
            OscType::String("100".into()), // all digits
        ],
    );
    assert_eq!(
        server.recv_until("/fail").args[0],
        OscType::String("/group_new".into())
    );

    // Neither node is there — `isGroup = -1` is how the record says so — and
    // the name still belongs to the group that took it first.
    for id in [101, 102] {
        server.send("/node_query", vec![OscType::Int(id)]);
        let info = server.recv_until("/node_query.reply").args;
        assert_eq!(info[0], OscType::Int(id));
        assert_eq!(info[4], OscType::Int(-1), "node {id} should not exist");
    }
    server.send("/group_query", vec![OscType::String("/mixer".into())]);
    assert_eq!(
        server.recv_until("/group_query.reply").args[1],
        OscType::Int(100)
    );
    server.quit();
}

/// `/transport_group` binds the group the transport governs. With one bound the
/// transport stops being an advisory the clients obey by choice and becomes
/// something the engine enforces; the query reply reports which group it is.
#[test]
fn transport_group_binds_and_unbinds() {
    let server = TestServer::spawn();

    // No grid needed: what a group is governed by is the rolling state, which
    // has nothing to do with beats. Binding the root group is legal and freezes
    // everything, which is why nothing else here does it.
    server.send("/transport_group", vec![OscType::Int(0)]);
    assert_eq!(
        server.recv_until("/done").args[0],
        OscType::String("/transport_group".into())
    );

    server.send(
        "/transport_set",
        vec![OscType::Long(0), OscType::Double(2.0)],
    );
    server.recv_until("/done");

    // An id that is not a group is refused rather than silently accepted.
    server.send("/transport_group", vec![OscType::Int(9999)]);
    let fail = server.recv_until("/fail");
    assert_eq!(fail.args[0], OscType::String("/transport_group".into()));
    assert!(
        format!("{:?}", fail.args[1]).contains("unknown group"),
        "got {:?}",
        fail.args
    );

    server.send(
        "/group_new",
        vec![OscType::Int(100), OscType::Int(0), OscType::Int(0)],
    );
    server.send("/transport_group", vec![OscType::Int(100)]);
    server.recv_until("/done");
    server.send("/transport_query", vec![]);
    assert_eq!(
        server.recv_until("/transport_query.reply").args[5],
        OscType::Int(100)
    );

    // A negative id unbinds.
    server.send("/transport_group", vec![OscType::Int(-1)]);
    server.recv_until("/done");
    server.send("/transport_query", vec![]);
    assert_eq!(
        server.recv_until("/transport_query.reply").args[5],
        OscType::Int(-1)
    );

    server.quit();
}

/// `/sched_atTransport` declares that its absolute sample is on the transport
/// axis. The declaration is a **check**: a packet that does not belong to the
/// governed subtree fails, rather than firing in the wrong place.
#[test]
fn sched_at_transport_checks_the_declared_axis() {
    let server = TestServer::spawn();

    let packet = encoder::encode(&OscPacket::Message(OscMessage {
        addr: "/node_run".into(),
        args: vec![OscType::Int(0), OscType::Int(1)],
    }))
    .unwrap();

    server.send(
        "/transport_set",
        vec![OscType::Long(0), OscType::Double(2.0)],
    );
    server.recv_until("/done");

    // No group bound: there is no transport axis to name.
    server.send(
        "/sched_atTransport",
        vec![OscType::Long(48_000), OscType::Blob(packet.clone())],
    );
    let fail = server.recv_until("/fail");
    assert!(
        format!("{:?}", fail.args[1]).contains("no group bound"),
        "got {:?}",
        fail.args
    );

    server.send(
        "/group_new",
        vec![OscType::Int(100), OscType::Int(0), OscType::Int(0)],
    );
    server.send("/transport_group", vec![OscType::Int(100)]);
    server.recv_until("/done");

    // The packet targets the root group, which is not governed: the client's
    // declared axis and the server's classification disagree, and that is
    // exactly what this command exists to catch.
    server.send(
        "/sched_atTransport",
        vec![OscType::Long(48_000), OscType::Blob(packet)],
    );
    let fail = server.recv_until("/fail");
    assert!(
        format!("{:?}", fail.args[1]).contains("not governed"),
        "got {:?}",
        fail.args
    );

    // A packet aimed at the governed group itself is accepted.
    let governed = encoder::encode(&OscPacket::Message(OscMessage {
        addr: "/node_run".into(),
        args: vec![OscType::Int(100), OscType::Int(1)],
    }))
    .unwrap();
    server.send(
        "/sched_atTransport",
        vec![OscType::Long(48_000), OscType::Blob(governed)],
    );
    assert_eq!(
        server.recv_until("/done").args[0],
        OscType::String("/sched_atTransport".into())
    );

    server.quit();
}

/// Freeing the governed group unbinds the transport: it cannot govern a node
/// that no longer exists, and leaving the binding would strand the next bind.
#[test]
fn freeing_the_governed_group_unbinds_the_transport() {
    let mut server = TestServer::spawn();

    // The unbind is announced on the push channel, so subscribe to it.
    server.send("/server_notify", vec![OscType::Int(1)]);
    server.recv_until("/done");
    server.send(
        "/transport_set",
        vec![OscType::Long(0), OscType::Double(2.0)],
    );
    server.recv_until("/done");
    server.send(
        "/group_new",
        vec![OscType::Int(100), OscType::Int(0), OscType::Int(0)],
    );
    server.send("/transport_group", vec![OscType::Int(100)]);
    server.recv_until("/done");
    // The bind is announced *after* its own /done, so the announcement is still
    // in flight here. Consume it: left in the socket, the tick below would match
    // it instead of the unbind's and stop ticking before the free had drained.
    assert_eq!(
        server.recv_until("/transport_query.reply").args[5],
        OscType::Int(100)
    );

    // The group's death reaches the network thread through the garbage FIFO,
    // which is drained on the server's own tick, so drive it until the push.
    server.send("/node_free", vec![OscType::Int(100)]);
    server.tick_until("/transport_query.reply");

    server.send("/transport_query", vec![]);
    assert_eq!(
        server.recv_until("/transport_query.reply").args[5],
        OscType::Int(-1)
    );

    server.quit();
}
