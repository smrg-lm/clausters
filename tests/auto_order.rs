//! M12: auto-sorted groups — bus-connection analysis, `/g_sortMode`,
//! `/g_queryTree`, `/g_dumpGraph`. Real UDP round-trips against a manually
//! ticked engine (no audio device), like `tests/osc.rs`.

use std::net::UdpSocket;
use std::thread::JoinHandle;
use std::time::Duration;

use clausters::osc::server::{OscServer, ServerInfo};
use clausters::rosc::{OscMessage, OscPacket, OscType, decoder, encoder};
use clausters::server::engine::{BLOCK_SIZE, Engine, engine_pair};
use serde_json::json;

const SR: f32 = 48_000.0;
const CHANNELS: usize = 2;

struct Server {
    addr: std::net::SocketAddr,
    thread: JoinHandle<std::io::Result<()>>,
    client: UdpSocket,
    engine: Engine,
}

impl Server {
    fn spawn() -> Self {
        let (engine, handle) = engine_pair(SR, CHANNELS);
        let info = ServerInfo {
            nominal_sample_rate: SR as f64,
            actual_sample_rate: SR as f64,
        };
        let mut server = OscServer::bind(("127.0.0.1", 0), info, handle).unwrap();
        let addr = server.local_addr().unwrap();
        let thread = std::thread::spawn(move || server.run());
        let client = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        Self {
            addr,
            thread,
            client,
            engine,
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

    fn recv(&self) -> OscMessage {
        let mut buf = [0u8; 65536];
        let (len, _) = self.client.recv_from(&mut buf).expect("reply timed out");
        match decoder::decode_udp(&buf[..len]).unwrap().1 {
            OscPacket::Message(msg) => msg,
            OscPacket::Bundle(_) => panic!("expected a message"),
        }
    }

    fn recv_until(&self, addr: &str) -> OscMessage {
        for _ in 0..100 {
            let msg = self.recv();
            if msg.addr == addr {
                return msg;
            }
        }
        panic!("never received {addr}");
    }

    fn d_recv(&self, def: &serde_json::Value) {
        self.send("/d_recv", vec![OscType::Blob(def.to_string().into_bytes())]);
        assert_eq!(self.recv_until("/done").args[0], OscType::String("/d_recv".into()));
    }

    /// Top-level child IDs of a flat group (synth children only).
    fn order_of(&self, group: i32) -> Vec<i32> {
        self.send("/g_queryTree", vec![OscType::Int(group)]);
        let reply = self.recv_until("/g_queryTree.reply");
        let mut order = Vec::new();
        // args: flag, groupID, numChildren, then per synth: id, -1, defname.
        let mut i = 3;
        while i < reply.args.len() {
            let OscType::Int(id) = reply.args[i] else { break };
            order.push(id);
            i += 3;
        }
        order
    }

    /// Polls until the group's order matches (the server thread is async).
    fn wait_for_order(&self, group: i32, expected: &[i32]) {
        for _ in 0..100 {
            if self.order_of(group) == expected {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "group {group} order never became {expected:?}, last seen {:?}",
            self.order_of(group)
        );
    }

    /// Ticks the engine and returns channel 0.
    fn render(&mut self, blocks: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; BLOCK_SIZE * CHANNELS];
        let mut buf = Vec::with_capacity(blocks * BLOCK_SIZE);
        for _ in 0..blocks {
            self.engine.process_block(&mut out);
            buf.extend(out.iter().step_by(CHANNELS).copied());
        }
        buf
    }

    fn quit(mut self) {
        self.send("/quit", vec![]);
        self.recv_until("/done");
        // Drain whatever the commands left behind so the engine half drops.
        self.render(2);
        self.thread.join().unwrap().unwrap();
    }
}

fn rms(buf: &[f32]) -> f32 {
    (buf.iter().map(|x| x * x).sum::<f32>() / buf.len() as f32).sqrt()
}

/// SinOsc(330)·0.2 summed into bus 16.
fn src_def() -> serde_json::Value {
    json!({
        "name": "src",
        "ugens": [
            {"kind": "SinOsc", "inputs": [{"const": 330.0}]},
            {"kind": "Mul", "inputs": [{"ugen": 0}, {"const": 0.2}]},
            {"kind": "Out", "inputs": [{"const": 16.0}, {"ugen": 1}]}
        ]
    })
}

/// Reads bus 16, halves it, replaces the bus contents (a classic insert fx).
fn fx_def() -> serde_json::Value {
    json!({
        "name": "fx",
        "ugens": [
            {"kind": "In", "inputs": [{"const": 16.0}]},
            {"kind": "Mul", "inputs": [{"ugen": 0}, {"const": 0.5}]},
            {"kind": "ReplaceOut", "inputs": [{"const": 16.0}, {"ugen": 1}]}
        ]
    })
}

/// Reads bus 16 into hardware out 0.
fn master_def() -> serde_json::Value {
    json!({
        "name": "master",
        "ugens": [
            {"kind": "In", "inputs": [{"const": 16.0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    })
}

#[test]
fn auto_group_reorders_a_reversed_chain() {
    let mut server = Server::spawn();
    server.d_recv(&src_def());
    server.d_recv(&fx_def());
    server.d_recv(&master_def());

    server.send(
        "/g_new",
        vec![OscType::Int(100), OscType::Int(0), OscType::Int(0)],
    );
    server.send("/g_sortMode", vec![OscType::Int(100), OscType::Int(1)]);
    // Deliberately reversed: master, then fx, then source — each /s_new
    // triggers a re-sort, so the final order must be src → fx → master.
    for (name, id) in [("master", 1001), ("fx", 1002), ("src", 1003)] {
        server.send(
            "/s_new",
            vec![
                OscType::String(name.into()),
                OscType::Int(id),
                OscType::Int(1),
                OscType::Int(100),
            ],
        );
    }
    server.wait_for_order(100, &[1003, 1002, 1001]);

    let out = server.render(50);
    // 0.2 · 0.5 sine: the chain is alive end to end in the same block.
    let expected = 0.1 * std::f32::consts::FRAC_1_SQRT_2;
    assert!(
        (rms(&out[BLOCK_SIZE..]) - expected).abs() < 0.005,
        "rms = {}, expected ≈ {expected}",
        rms(&out[BLOCK_SIZE..])
    );
    server.quit();
}

#[test]
fn n_mapa_adds_a_read_edge_and_resorts() {
    // `src` writes bus 16; `default` reads no bus statically, so an auto group
    // leaves the two in insertion order. Mapping default's freq to bus 16 with
    // /n_mapa makes it read that bus — a writer-before-reader edge that must
    // re-sort src ahead of it (M11 feeding the M12/M13 analysis).
    let server = Server::spawn();
    server.d_recv(&src_def());
    server.send(
        "/g_new",
        vec![OscType::Int(100), OscType::Int(0), OscType::Int(0)],
    );
    server.send("/g_sortMode", vec![OscType::Int(100), OscType::Int(1)]);
    // Reader first, writer second: nothing connects them yet.
    server.send(
        "/s_new",
        vec![
            OscType::String("default".into()),
            OscType::Int(1001),
            OscType::Int(1),
            OscType::Int(100),
        ],
    );
    server.send(
        "/s_new",
        vec![
            OscType::String("src".into()),
            OscType::Int(1002),
            OscType::Int(1),
            OscType::Int(100),
        ],
    );
    server.wait_for_order(100, &[1001, 1002]);

    // /n_mapa freq -> bus 16: now 1001 reads what 1002 writes.
    server.send(
        "/n_mapa",
        vec![OscType::Int(1001), OscType::Int(0), OscType::Int(16)],
    );
    server.wait_for_order(100, &[1002, 1001]);

    // Unmapping drops the read edge. The sort is stable, so the now
    // unconstrained pair keeps its current order rather than snapping back —
    // re-sorting must not deadlock or shuffle it.
    server.send(
        "/n_mapa",
        vec![OscType::Int(1001), OscType::Int(0), OscType::Int(-1)],
    );
    server.wait_for_order(100, &[1002, 1001]);
    server.quit();
}

#[test]
fn manual_group_keeps_the_reversed_chain_silent() {
    let mut server = Server::spawn();
    server.d_recv(&src_def());
    server.d_recv(&fx_def());
    server.d_recv(&master_def());
    server.send(
        "/g_new",
        vec![OscType::Int(100), OscType::Int(0), OscType::Int(0)],
    );
    for (name, id) in [("master", 1001), ("fx", 1002), ("src", 1003)] {
        server.send(
            "/s_new",
            vec![
                OscType::String(name.into()),
                OscType::Int(id),
                OscType::Int(1),
                OscType::Int(100),
            ],
        );
    }
    server.wait_for_order(100, &[1001, 1002, 1003]);
    let out = server.render(50);
    assert!(
        out.iter().all(|s| *s == 0.0),
        "buses are cleared per block: the reversed manual chain must be silent"
    );
    server.quit();
}

#[test]
fn g_sort_mode_sorts_existing_children_and_can_be_disabled() {
    let mut server = Server::spawn();
    server.d_recv(&src_def());
    server.d_recv(&master_def());
    server.send(
        "/g_new",
        vec![OscType::Int(100), OscType::Int(0), OscType::Int(0)],
    );
    for (name, id) in [("master", 1001), ("src", 1002)] {
        server.send(
            "/s_new",
            vec![
                OscType::String(name.into()),
                OscType::Int(id),
                OscType::Int(1),
                OscType::Int(100),
            ],
        );
    }
    server.wait_for_order(100, &[1001, 1002]);
    assert!(server.render(20).iter().all(|s| *s == 0.0));

    // Enabling sorts what is already there…
    server.send("/g_sortMode", vec![OscType::Int(100), OscType::Int(1)]);
    server.wait_for_order(100, &[1002, 1001]);
    let out = server.render(50);
    assert!(rms(&out[BLOCK_SIZE..]) > 0.1, "sorted chain must be audible");

    // …and disabling re-enables manual moves.
    server.send("/g_sortMode", vec![OscType::Int(100), OscType::Int(0)]);
    server.send(
        "/n_before",
        vec![OscType::Int(1001), OscType::Int(1002)],
    );
    server.wait_for_order(100, &[1001, 1002]);
    server.quit();
}

#[test]
fn manual_moves_fail_inside_auto_groups() {
    let server = Server::spawn();
    server.send(
        "/g_new",
        vec![OscType::Int(100), OscType::Int(0), OscType::Int(0)],
    );
    server.send("/g_sortMode", vec![OscType::Int(100), OscType::Int(1)]);
    for id in [1001, 1002] {
        server.send(
            "/s_new",
            vec![
                OscType::String("default".into()),
                OscType::Int(id),
                OscType::Int(1),
                OscType::Int(100),
            ],
        );
    }
    server.send(
        "/n_before",
        vec![OscType::Int(1002), OscType::Int(1001)],
    );
    let reply = server.recv_until("/fail");
    assert_eq!(reply.args[0], OscType::String("/n_before".into()));
    server.quit();
}

#[test]
fn g_sort_mode_rejects_missing_or_non_groups() {
    let server = Server::spawn();
    server.send("/g_sortMode", vec![OscType::Int(999), OscType::Int(1)]);
    assert_eq!(
        server.recv_until("/fail").args[0],
        OscType::String("/g_sortMode".into())
    );
    server.send(
        "/s_new",
        vec![
            OscType::String("default".into()),
            OscType::Int(1001),
            OscType::Int(1),
            OscType::Int(0),
        ],
    );
    server.send("/g_sortMode", vec![OscType::Int(1001), OscType::Int(1)]);
    assert_eq!(
        server.recv_until("/fail").args[0],
        OscType::String("/g_sortMode".into())
    );
    server.quit();
}

#[test]
fn query_tree_reports_structure_and_controls() {
    let server = Server::spawn();
    server.send(
        "/g_new",
        vec![OscType::Int(100), OscType::Int(0), OscType::Int(0)],
    );
    server.send(
        "/s_new",
        vec![
            OscType::String("default".into()),
            OscType::Int(1001),
            OscType::Int(1),
            OscType::Int(100),
            OscType::String("freq".into()),
            OscType::Float(220.0),
        ],
    );
    // Poll until the mirror has it, then check the full flag-1 layout.
    server.wait_for_order(100, &[1001]);
    server.send("/g_queryTree", vec![OscType::Int(100), OscType::Int(1)]);
    let reply = server.recv_until("/g_queryTree.reply");
    let expected: Vec<OscType> = vec![
        OscType::Int(1),            // flag
        OscType::Int(100),          // queried group
        OscType::Int(1),            // its child count
        OscType::Int(1001),         // the synth
        OscType::Int(-1),           // synth marker
        OscType::String("default".into()),
        OscType::Int(2),            // control count
        OscType::String("freq".into()),
        OscType::Float(220.0),      // /s_new override, mirrored
        OscType::String("amp".into()),
        OscType::Float(0.2),        // default
    ];
    assert_eq!(reply.args, expected);
    server.quit();
}

#[test]
fn dynamic_bus_indexes_are_reported_and_act_as_barriers() {
    let server = Server::spawn();
    // The In bus index comes from a signal: not statically analyzable.
    server.d_recv(&json!({
        "name": "dynread",
        "ugens": [
            {"kind": "SinOsc", "inputs": [{"const": 1.0}]},
            {"kind": "In", "inputs": [{"ugen": 0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]
    }));
    server.d_recv(&src_def());
    server.d_recv(&master_def());
    server.send(
        "/g_new",
        vec![OscType::Int(100), OscType::Int(0), OscType::Int(0)],
    );
    server.send("/g_sortMode", vec![OscType::Int(100), OscType::Int(1)]);
    // master, then the barrier, then src: src→master would normally re-sort,
    // but nothing may cross the dynamic node, so the order must hold.
    for (name, id) in [("master", 1001), ("dynread", 1002), ("src", 1003)] {
        server.send(
            "/s_new",
            vec![
                OscType::String(name.into()),
                OscType::Int(id),
                OscType::Int(1),
                OscType::Int(100),
            ],
        );
    }
    server.wait_for_order(100, &[1001, 1002, 1003]);

    server.send("/g_dumpGraph", vec![OscType::Int(100)]);
    let reply = server.recv_until("/g_dumpGraph.reply");
    let OscType::String(dump) = &reply.args[1] else {
        panic!("expected a string dump");
    };
    assert!(dump.contains("dynamic"), "dump must flag the barrier:\n{dump}");
    assert!(dump.contains("group 100 (auto)"), "dump header:\n{dump}");
    server.quit();
}

#[test]
fn feedback_cycles_keep_insertion_order() {
    let server = Server::spawn();
    // Two cross-coupled nodes: a reads 16 writes 17, b reads 17 writes 16.
    for (name, read, write) in [("xa", 16.0, 17.0), ("xb", 17.0, 16.0)] {
        server.d_recv(&json!({
            "name": name,
            "ugens": [
                {"kind": "In", "inputs": [{"const": read}]},
                {"kind": "Out", "inputs": [{"const": write}, {"ugen": 0}]}
            ]
        }));
    }
    server.d_recv(&src_def());
    server.send(
        "/g_new",
        vec![OscType::Int(100), OscType::Int(0), OscType::Int(0)],
    );
    server.send("/g_sortMode", vec![OscType::Int(100), OscType::Int(1)]);
    for (name, id) in [("xa", 1001), ("xb", 1002), ("src", 1003)] {
        server.send(
            "/s_new",
            vec![
                OscType::String(name.into()),
                OscType::Int(id),
                OscType::Int(1),
                OscType::Int(100),
            ],
        );
    }
    // The source feeds the loop, so it sorts first; the cycle pair keeps
    // its insertion order (one block of feedback delay, by design).
    server.wait_for_order(100, &[1003, 1001, 1002]);
    server.quit();
}

#[test]
fn n_set_on_a_bus_control_resorts() {
    let mut server = Server::spawn();
    server.d_recv(&json!({
        "name": "srcvar",
        "controls": [{"name": "bus", "default": 20.0}],
        "ugens": [
            {"kind": "SinOsc", "inputs": [{"const": 330.0}]},
            {"kind": "Mul", "inputs": [{"ugen": 0}, {"const": 0.2}]},
            {"kind": "Out", "inputs": [{"control": 0}, {"ugen": 1}]}
        ]
    }));
    server.d_recv(&master_def());
    server.send(
        "/g_new",
        vec![OscType::Int(100), OscType::Int(0), OscType::Int(0)],
    );
    server.send("/g_sortMode", vec![OscType::Int(100), OscType::Int(1)]);
    for (name, id) in [("master", 1001), ("srcvar", 1002)] {
        server.send(
            "/s_new",
            vec![
                OscType::String(name.into()),
                OscType::Int(id),
                OscType::Int(1),
                OscType::Int(100),
            ],
        );
    }
    // Writing bus 20, nobody listens: no dependency, insertion order holds.
    server.wait_for_order(100, &[1001, 1002]);
    assert!(server.render(20).iter().all(|s| *s == 0.0));

    // Retargeting the source onto the master's bus re-analyzes and re-sorts.
    server.send(
        "/n_set",
        vec![
            OscType::Int(1002),
            OscType::String("bus".into()),
            OscType::Float(16.0),
        ],
    );
    server.wait_for_order(100, &[1002, 1001]);
    let out = server.render(50);
    assert!(rms(&out[BLOCK_SIZE..]) > 0.1, "re-sorted chain must be audible");
    server.quit();
}

/// The renderer shares the translator, so scores get auto-sorting too.
#[test]
fn nrt_scores_support_g_sort_mode() {
    use clausters::server::render::{RenderConfig, Score, render_to_vec};

    let msg = |addr: &str, args: Vec<OscType>| OscMessage {
        addr: addr.into(),
        args,
    };
    let events = vec![
        (
            0.0,
            vec![
                msg("/d_recv", vec![OscType::Blob(src_def().to_string().into_bytes())]),
                msg("/d_recv", vec![OscType::Blob(master_def().to_string().into_bytes())]),
                msg(
                    "/g_new",
                    vec![OscType::Int(100), OscType::Int(0), OscType::Int(0)],
                ),
                msg("/g_sortMode", vec![OscType::Int(100), OscType::Int(1)]),
                // Reversed on purpose, again.
                msg(
                    "/s_new",
                    vec![
                        OscType::String("master".into()),
                        OscType::Int(1001),
                        OscType::Int(1),
                        OscType::Int(100),
                    ],
                ),
                msg(
                    "/s_new",
                    vec![
                        OscType::String("src".into()),
                        OscType::Int(1002),
                        OscType::Int(1),
                        OscType::Int(100),
                    ],
                ),
            ],
        ),
        (0.1, vec![msg("/n_free", vec![OscType::Int(1001)])]),
    ];
    let score = Score::new(events).unwrap();
    let cfg = RenderConfig {
        sample_rate: SR as f64,
        channels: 1,
        ..RenderConfig::default()
    };
    let (out, _) = render_to_vec(&score, &cfg).expect("render must succeed");
    let expected = 0.2 * std::f32::consts::FRAC_1_SQRT_2;
    assert!(
        (rms(&out[BLOCK_SIZE..]) - expected).abs() < 0.005,
        "auto-sorted score must be audible offline: rms = {}",
        rms(&out[BLOCK_SIZE..])
    );
}

/// Faust synths expose their bus usage through the reserved out/in controls.
#[cfg(feature = "faust")]
#[test]
fn faust_synths_sort_by_their_reserved_buses() {
    let mut server = Server::spawn();
    server.d_recv(&master_def());
    server.send(
        "/d_faust",
        vec![
            OscType::String("fsrc".into()),
            OscType::String("import(\"stdfaust.lib\"); process = os.osc(330) * 0.2;".into()),
        ],
    );
    assert_eq!(
        server.recv_until("/done").args[0],
        OscType::String("/d_faust".into())
    );
    server.send(
        "/g_new",
        vec![OscType::Int(100), OscType::Int(0), OscType::Int(0)],
    );
    server.send("/g_sortMode", vec![OscType::Int(100), OscType::Int(1)]);
    // master first, then the Faust source writing bus 16: must sort first.
    server.send(
        "/s_new",
        vec![
            OscType::String("master".into()),
            OscType::Int(1001),
            OscType::Int(1),
            OscType::Int(100),
        ],
    );
    server.send(
        "/s_new",
        vec![
            OscType::String("fsrc".into()),
            OscType::Int(1002),
            OscType::Int(1),
            OscType::Int(100),
            OscType::String("out".into()),
            OscType::Float(16.0),
        ],
    );
    server.wait_for_order(100, &[1002, 1001]);
    let out = server.render(50);
    assert!(rms(&out[BLOCK_SIZE..]) > 0.1, "faust source must reach the master");
    server.quit();
}
