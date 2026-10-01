//! A UGen's replies reach whoever they are for, live and offline alike: a
//! fault goes to every `/server_notify` client as `/node_fault` (the console
//! is not where a client looks), and a `Poll` posts the same line in an
//! offline render as on the live server.

#![cfg(feature = "synth")]

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clausters::osc::server::{OscServer, ServerInfo};
use clausters::rosc::{OscMessage, OscPacket, OscType, encoder};
use clausters::server::engine::{BLOCK_SIZE, engine_pair_full};
use clausters::server::ipc::{IpcPeer, Role, Segment};
use clausters::server::render::{OscScore, RenderConfig, render_to_vec};
use clausters::synthdef::SynthDefSpec;
use serde_json::json;

const SR: f32 = 48_000.0;
const PEER: u32 = 1;

fn msg(addr: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage {
        addr: addr.into(),
        args,
    }
}

fn def(ugens: serde_json::Value) -> OscType {
    OscType::Blob(
        serde_json::to_vec(
            &serde_json::from_value::<SynthDefSpec>(json!({"name": "d", "ugens": ugens})).unwrap(),
        )
        .unwrap(),
    )
}

/// A `Conv` holding two partitions over a kernel of four, in buffer 1.
fn too_long() -> Vec<OscMessage> {
    vec![
        msg(
            "/buffer_alloc",
            vec![OscType::Int(0), OscType::Int(1000), OscType::Int(1)],
        ),
        msg(
            "/buffer_alloc",
            vec![OscType::Int(1), OscType::Int(2 + 4 * 512), OscType::Int(1)],
        ),
        msg(
            "/buffer_gen",
            vec![
                OscType::Int(1),
                OscType::String("prepare_partconv".into()),
                OscType::Int(512),
                OscType::Int(0),
            ],
        ),
        msg(
            "/def_send",
            vec![
                OscType::String("synth".into()),
                def(json!([
                    {"kind": "WhiteNoise", "inputs": []},
                    {"kind": "Conv", "inputs": [{"ugen": 0}, {"const": 1.0}],
                     "fft_size": 512, "partitions": 2},
                    {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
                ])),
            ],
        ),
    ]
}

/// A live server tells its `/server_notify` clients about a fault: the node,
/// the UGen's kind, the code and the sentence the server logs.
#[test]
fn a_fault_reaches_the_notified_clients() {
    let segment = Segment::in_memory();
    let (mut engine, handle) = engine_pair_full(
        SR,
        2,
        0,
        Some(Arc::clone(&segment)),
        128,
        1024,
        clausters::dsp::Limits::default(),
    );
    let info = ServerInfo {
        nominal_sample_rate: SR as f64,
        actual_sample_rate: SR as f64,
    };
    let mut server = OscServer::headless(info, handle, 0.0);
    server
        .attach_ipc(IpcPeer::new(Arc::clone(&segment), Role::Server))
        .unwrap();
    let client = IpcPeer::new(segment, Role::Client);
    let send = |m: OscMessage| {
        assert!(client.push(PEER, &encoder::encode(&OscPacket::Message(m)).unwrap()));
    };

    send(msg("/server_notify", vec![OscType::Int(1)]));
    // A `/buffer_gen` reads the buffer an earlier `/buffer_alloc` made only
    // once that has completed: the prepare goes a hundred blocks later.
    let mut setup = too_long();
    let prepare = setup.remove(2);
    for m in setup {
        send(m);
    }
    let mut buf = vec![0u8; 65536];
    let mut fault = None;
    let mut sent_synth = false;
    for step in 0..2000 {
        server.step();
        engine.process_block(&mut vec![0.0f32; BLOCK_SIZE * 2]);
        if step == 100 {
            send(prepare.clone());
        }
        // The kernel is built on the NRT queue: once it is, the synth.
        if !sent_synth && step == 200 {
            send(msg(
                "/synth_new",
                vec![
                    OscType::String("d".into()),
                    OscType::Int(1000),
                    OscType::Int(0),
                    OscType::Int(0),
                ],
            ));
            sent_synth = true;
        }
        while let Some((_, len)) = client.try_pop(&mut buf) {
            if let Ok(OscPacket::Message(m)) = clausters::osc::decode_packet(&buf[..len])
                && m.addr == "/node_fault"
            {
                fault = Some(m);
            }
        }
        if fault.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let fault = fault.expect("a /node_fault reached the client");
    assert_eq!(fault.args[0], OscType::Int(1000));
    assert_eq!(fault.args[1], OscType::String("Conv".into()));
    assert_eq!(
        fault.args[2],
        OscType::Int(clausters::dsp::conv::fault::TOO_LONG)
    );
    let OscType::String(sentence) = &fault.args[3] else {
        panic!("{fault:?}")
    };
    assert!(
        sentence.contains("has 4 partitions and this Conv holds 2"),
        "{sentence}"
    );
}

/// A writer the test reads back.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// An offline render posts a `Poll`'s line as the live server does: the
/// label and the value, on the OSC log target at `info`.
#[test]
fn an_offline_poll_posts_its_line() {
    let log = Captured::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_max_level(tracing::Level::INFO)
        .without_time()
        .finish();
    let events = vec![
        (
            0.0,
            vec![
                msg(
                    "/def_send",
                    vec![
                        OscType::String("synth".into()),
                        def(json!([
                            {"kind": "Impulse", "inputs": [{"const": 10.0}]},
                            {"kind": "Poll",
                             "inputs": [{"ugen": 0}, {"const": 0.25}, {"const": -1.0}],
                             "label": "level"}
                        ])),
                    ],
                ),
                msg(
                    "/synth_new",
                    vec![
                        OscType::String("d".into()),
                        OscType::Int(100),
                        OscType::Int(0),
                        OscType::Int(0),
                    ],
                ),
            ],
        ),
        (0.05, vec![msg("/node_free", vec![OscType::Int(100)])]),
    ];
    let cfg = RenderConfig {
        sample_rate: SR as f64,
        channels: 1,
        ..RenderConfig::default()
    };
    tracing::subscriber::with_default(subscriber, || {
        render_to_vec(&OscScore::new(events).unwrap(), &cfg).unwrap();
    });
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(text.contains("level: 0.25"), "{text}");
}

/// A render keeps what it logged at `info` and above and hands it back on its
/// stats -- the one way a render in a process with no logger of its own (the
/// embed ABI, the page) can tell its caller that the engine rejected a node,
/// or what a `Poll` said.
#[test]
fn a_render_hands_back_what_it_logged() {
    let poll = def(json!([
        {"kind": "Impulse", "inputs": [{"const": 10.0}]},
        {"kind": "Poll",
         "inputs": [{"ugen": 0}, {"const": 0.25}, {"const": -1.0}],
         "label": "level"}
    ]));
    let synth = |id: i32| {
        msg(
            "/synth_new",
            vec![
                OscType::String("d".into()),
                OscType::Int(id),
                OscType::Int(0),
                OscType::Int(0),
            ],
        )
    };
    let events = vec![
        (
            0.0,
            vec![
                msg("/def_send", vec![OscType::String("synth".into()), poll]),
                synth(100),
                // The same id again: the engine rejects it.
                synth(100),
            ],
        ),
        (0.05, vec![msg("/node_free", vec![OscType::Int(100)])]),
    ];
    let cfg = RenderConfig {
        sample_rate: SR as f64,
        channels: 1,
        ..RenderConfig::default()
    };
    let (_, stats) = render_to_vec(&OscScore::new(events).unwrap(), &cfg).unwrap();
    let has = |level: tracing::Level, text: &str| {
        stats
            .log
            .iter()
            .any(|l| l.level == level && l.message.contains(text))
    };
    assert!(
        has(tracing::Level::WARN, "rejected node 100"),
        "{:?}",
        stats.log
    );
    assert!(has(tracing::Level::INFO, "level: 0.25"), "{:?}", stats.log);
}
