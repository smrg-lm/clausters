//! M13: parallel processing of `/g_parallel` groups. The central claim
//! under test: parallel execution is **bit-identical** to sequential
//! execution — stages only batch children with pairwise disjoint bus
//! usage, so worker interleaving can never change a sample.

#![cfg(feature = "synth")]

use clausters::osc::translate::CmdTranslator;
use clausters::rosc::{OscMessage, OscType};
use clausters::server::engine::{BLOCK_SIZE, Engine, EngineHandle, engine_pair_with_workers};
use serde_json::json;

const SR: f32 = 48_000.0;
const CHANNELS: usize = 2;

fn msg(addr: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage {
        addr: addr.into(),
        args,
    }
}

fn s_new(name: &str, id: i32, target: i32, ctls: &[(&str, f32)]) -> OscMessage {
    let mut args = vec![
        OscType::String(name.into()),
        OscType::Int(id),
        OscType::Int(1), // tail
        OscType::Int(target),
    ];
    for (k, v) in ctls {
        args.push(OscType::String((*k).into()));
        args.push(OscType::Float(*v));
    }
    msg("/s_new", args)
}

/// A source summing `Sine(freq)·0.2` into a constant bus.
fn src_def(name: &str, bus: f32, freq: f32) -> OscMessage {
    let def = json!({
        "name": name,
        "ugens": [
            {"kind": "Sine", "inputs": [{"const": freq}]},
            {"kind": "Mul", "inputs": [{"ugen": 0}, {"const": 0.2}]},
            {"kind": "Out", "inputs": [{"const": bus}, {"ugen": 1}]}
        ]
    });
    msg("/d_recv", vec![OscType::Blob(def.to_string().into_bytes())])
}

/// An insert fx: halves `bus` in place (read + ReplaceOut).
fn fx_def(name: &str, bus: f32) -> OscMessage {
    let def = json!({
        "name": name,
        "ugens": [
            {"kind": "In", "inputs": [{"const": bus}]},
            {"kind": "Mul", "inputs": [{"ugen": 0}, {"const": 0.5}]},
            {"kind": "ReplaceOut", "inputs": [{"const": bus}, {"ugen": 1}]}
        ]
    });
    msg("/d_recv", vec![OscType::Blob(def.to_string().into_bytes())])
}

/// The torture graph, sent identically to every engine under test:
/// a parallel group holding disjoint sources (one stage), a nested group
/// as a unit, insert fx on two buses (second stage), two masters summing
/// into the hardware bus (write conflict ⇒ serialized), and a dynamic
/// reader (signal-driven bus index ⇒ runs alone).
fn torture_graph() -> Vec<OscMessage> {
    let mut m = vec![
        src_def("src16", 16.0, 220.0),
        src_def("src17", 17.0, 330.0),
        src_def("src18", 18.0, 440.0),
        src_def("src20", 20.0, 550.0),
        src_def("src21", 21.0, 660.0),
        fx_def("fx16", 16.0),
        fx_def("fx17", 17.0),
    ];
    // Master: mixes buses 16..18 into hardware out 0.
    let mix = json!({
        "name": "mix",
        "ugens": [
            {"kind": "In", "inputs": [{"const": 16.0}]},
            {"kind": "In", "inputs": [{"const": 17.0}]},
            {"kind": "In", "inputs": [{"const": 18.0}]},
            {"kind": "Add", "inputs": [{"ugen": 0}, {"ugen": 1}]},
            {"kind": "Add", "inputs": [{"ugen": 3}, {"ugen": 2}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 4}]}
        ]
    });
    m.push(msg(
        "/d_recv",
        vec![OscType::Blob(mix.to_string().into_bytes())],
    ));
    // Second master: nested-group buses into out 0 too (conflicting write).
    let mix2 = json!({
        "name": "mix2",
        "ugens": [
            {"kind": "In", "inputs": [{"const": 20.0}]},
            {"kind": "In", "inputs": [{"const": 21.0}]},
            {"kind": "Add", "inputs": [{"ugen": 0}, {"ugen": 1}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 2}]}
        ]
    });
    m.push(msg(
        "/d_recv",
        vec![OscType::Blob(mix2.to_string().into_bytes())],
    ));
    // Dynamic: the In bus index is a signal — must run alone, untouched.
    let dynread = json!({
        "name": "dynread",
        "ugens": [
            {"kind": "Sine", "inputs": [{"const": 0.25}]},
            {"kind": "In", "inputs": [{"ugen": 0}]},
            {"kind": "Out", "inputs": [{"const": 1.0}, {"ugen": 1}]}
        ]
    });
    m.push(msg(
        "/d_recv",
        vec![OscType::Blob(dynread.to_string().into_bytes())],
    ));
    // A source whose output bus is a control (re-analyzed on /n_set later).
    let srcvar = json!({
        "name": "srcvar",
        "controls": [{"name": "bus", "default": 19.0}],
        "ugens": [
            {"kind": "Sine", "inputs": [{"const": 110.0}]},
            {"kind": "Mul", "inputs": [{"ugen": 0}, {"const": 0.1}]},
            {"kind": "Out", "inputs": [{"control": 0}, {"ugen": 1}]}
        ]
    });
    m.push(msg(
        "/d_recv",
        vec![OscType::Blob(srcvar.to_string().into_bytes())],
    ));

    m.push(msg(
        "/g_new",
        vec![OscType::Int(100), OscType::Int(0), OscType::Int(0)],
    ));
    m.push(msg("/g_parallel", vec![OscType::Int(100), OscType::Int(1)]));
    // Dependency-correct insertion order (M12 could do this; here it is
    // explicit so the test only exercises M13).
    m.push(s_new("src16", 1001, 100, &[]));
    m.push(s_new("src17", 1002, 100, &[]));
    m.push(s_new("src18", 1003, 100, &[]));
    m.push(s_new("srcvar", 1004, 100, &[]));
    // Nested group with two more disjoint sources: one unit for the stage.
    m.push(msg(
        "/g_new",
        vec![OscType::Int(200), OscType::Int(1), OscType::Int(100)],
    ));
    m.push(s_new("src20", 2001, 200, &[]));
    m.push(s_new("src21", 2002, 200, &[]));
    // Stage 2: the two insert fx (disjoint buses 16 and 17).
    m.push(s_new("fx16", 1005, 100, &[]));
    m.push(s_new("fx17", 1006, 100, &[]));
    // Stage 3: conflicting writers to out 0 — must serialize, in order.
    m.push(s_new("mix", 1007, 100, &[]));
    m.push(s_new("mix2", 1008, 100, &[]));
    // Barrier: dynamic bus index.
    m.push(s_new("dynread", 1009, 100, &[]));
    m
}

struct Rig {
    engine: Engine,
    handle: EngineHandle,
    translator: CmdTranslator,
}

impl Rig {
    fn new(workers: usize, messages: &[OscMessage]) -> Self {
        let (engine, handle) = engine_pair_with_workers(SR, CHANNELS, workers);
        let mut rig = Self {
            engine,
            handle,
            translator: CmdTranslator::new(SR),
        };
        rig.send(messages);
        rig
    }

    fn send(&mut self, messages: &[OscMessage]) {
        for m in messages {
            if m.addr == "/d_recv" {
                self.translator.d_recv(&m.args).unwrap();
                continue;
            }
            let mut cmds = Vec::new();
            self.translator.translate(m, &mut cmds).unwrap();
            for cmd in cmds {
                self.handle.send(cmd).ok().unwrap();
            }
        }
    }

    fn render(&mut self, blocks: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; BLOCK_SIZE * CHANNELS];
        let mut buf = Vec::with_capacity(blocks * BLOCK_SIZE * CHANNELS);
        for _ in 0..blocks {
            self.engine.process_block(&mut out);
            buf.extend_from_slice(&out);
        }
        buf
    }
}

#[test]
fn parallel_output_is_bit_identical_to_sequential() {
    let graph = torture_graph();
    let mut sequential = Rig::new(0, &graph);
    let mut parallel = Rig::new(3, &graph);

    let a = sequential.render(40);
    let b = parallel.render(40);
    assert!(a.iter().any(|s| *s != 0.0), "the graph must be audible");
    assert!(a == b, "parallel rendering must be bit-identical");

    // Retarget the control-driven source onto a contended bus: the engine
    // masks update via Cmd::SetUsage and the partition adapts — still
    // bit-identical.
    let retune = vec![msg(
        "/n_set",
        vec![
            OscType::Int(1004),
            OscType::String("bus".into()),
            OscType::Float(16.0),
        ],
    )];
    sequential.send(&retune);
    parallel.send(&retune);
    let a = sequential.render(40);
    let b = parallel.render(40);
    assert!(a == b, "bit-identical after the usage change too");
}

#[test]
fn workers_survive_many_blocks_and_drop_cleanly() {
    // Exercises the publish/park/unpark cycle far past the spin budget and
    // then the shutdown path (a hang here fails the test by timeout).
    let graph = torture_graph();
    let mut rig = Rig::new(2, &graph);
    for _ in 0..20 {
        let out = rig.render(50);
        assert!(out.iter().any(|s| *s != 0.0));
        std::thread::sleep(std::time::Duration::from_millis(30)); // let workers park
    }
}

#[test]
fn g_parallel_rejects_missing_or_non_groups() {
    let mut translator = CmdTranslator::new(SR);
    let mut cmds = Vec::new();
    let err = translator
        .translate(
            &msg("/g_parallel", vec![OscType::Int(999), OscType::Int(1)]),
            &mut cmds,
        )
        .unwrap_err();
    assert!(err.contains("not found"), "{err}");

    translator
        .translate(&s_new("default", 1001, 0, &[]), &mut cmds)
        .unwrap();
    let err = translator
        .translate(
            &msg("/g_parallel", vec![OscType::Int(1001), OscType::Int(1)]),
            &mut cmds,
        )
        .unwrap_err();
    assert!(err.contains("not a group"), "{err}");
}

/// NRT renders with workers must equal the sequential render bit for bit
/// (`--workers` in `clausters --nrt` only changes the wall-clock time).
#[test]
fn nrt_render_with_workers_is_bit_identical() {
    use clausters::server::render::{RenderConfig, Score, render_to_vec};

    let graph = torture_graph();
    let events = vec![
        (0.0, graph),
        (0.25, vec![msg("/n_free", vec![OscType::Int(100)])]),
    ];
    let score = Score::new(events).unwrap();
    let base = RenderConfig {
        sample_rate: SR as f64,
        channels: 2,
        workers: 0,
    };
    let (a, _) = render_to_vec(&score, &base).unwrap();
    let (b, _) = render_to_vec(&score, &RenderConfig { workers: 2, ..base }).unwrap();
    assert!(a.iter().any(|s| *s != 0.0));
    assert!(a == b, "offline parallel render must be bit-identical");
}
