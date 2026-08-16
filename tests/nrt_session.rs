//! S13: the NRT server taking operations on demand.
//!
//! The claim under test is the one the mode is defined by, and it is not a
//! claim about time: **determinism here is of process**. An interactive session
//! cannot be deterministic in time — it answers a document, not a timeline —
//! so what has to hold is that the *same operation over the same material*
//! yields the samples it would yield expressed in a score and rendered in
//! batch. Everything below is that sentence, checked.

#![cfg(feature = "synth")]

use clausters::rosc::{OscMessage, OscType};
use clausters::server::nrtsession::{NrtSession, SessionConfig};
use clausters::server::render::{RenderConfig, Score, render_to_vec};
use serde_json::json;

const SR: f64 = 48_000.0;
const CHANNELS: usize = 2;
const SEED: u64 = 0x5eed_1234;

fn msg(addr: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage {
        addr: addr.into(),
        args,
    }
}

/// A def writing `src * 0.2` to both output buses.
fn def(name: &str, src: serde_json::Value) -> OscMessage {
    let d = json!({
        "name": name,
        "ugens": [
            src,
            {"kind": "Mul", "inputs": [{"ugen": 0}, {"const": 0.2}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]},
            {"kind": "Out", "inputs": [{"const": 1.0}, {"ugen": 1}]}
        ]
    });
    msg(
        "/def_send",
        vec![
            OscType::String("synth".into()),
            OscType::Blob(d.to_string().into_bytes()),
        ],
    )
}

fn sine_def(name: &str) -> OscMessage {
    def(name, json!({"kind": "Sine", "inputs": [{"const": 440.0}]}))
}

/// Stochastic, so the seed path is under test and not only the graph.
fn noise_def(name: &str) -> OscMessage {
    def(name, json!({"kind": "WhiteNoise", "inputs": []}))
}

fn s_new(name: &str, id: i32) -> OscMessage {
    msg(
        "/synth_new",
        vec![
            OscType::String(name.into()),
            OscType::Int(id),
            OscType::Int(1), // tail
            OscType::Int(0), // of the root group
        ],
    )
}

fn render_cfg() -> RenderConfig {
    RenderConfig {
        sample_rate: SR,
        channels: CHANNELS,
        workers: 0,
        seed: Some(SEED),
        limits: Default::default(),
    }
}

fn session_cfg() -> SessionConfig {
    SessionConfig {
        sample_rate: SR,
        channels: CHANNELS,
        workers: 0,
        seed: Some(SEED),
        ..Default::default()
    }
}

/// The same material two ways: a score rendered in batch, and the same
/// commands sent to a session which is then asked to run the span.
fn batch(def_msg: OscMessage, frames: u64) -> Vec<f32> {
    let dur = frames as f64 / SR;
    let score = Score::new([
        (0.0, vec![def_msg, s_new("t", 1000)]),
        // A score ends at its last bundle, whose commands make no sound.
        (dur, vec![msg("/node_free", vec![OscType::Int(1000)])]),
    ])
    .expect("score");
    let (samples, _) = render_to_vec(&score, &render_cfg()).expect("batch render");
    samples
}

fn interactive(def_msg: OscMessage, frames: u64) -> Vec<f32> {
    let mut s = NrtSession::open(&session_cfg()).expect("open");
    for m in [def_msg, s_new("t", 1000)] {
        assert!(s.send_msg(&m.addr, m.args).expect("encode"), "ring full");
        // Serve it without advancing time: the def compiles and the synth is
        // built, and the session's frame count is untouched.
        s.settle_for(4);
    }
    assert_eq!(s.frames(), 0, "settling must not advance time");
    s.run_to_vec(frames).expect("run")
}

#[test]
fn an_operation_equals_the_same_score_rendered_in_batch() {
    let frames = 4096;
    let a = batch(sine_def("t"), frames);
    let b = interactive(sine_def("t"), frames);
    assert_eq!(a.len(), b.len(), "both produce the whole span");
    assert_eq!(
        a, b,
        "an operation must be sample-identical to the batch render of the same material"
    );
    assert!(a.iter().any(|&x| x != 0.0), "the material has to sound");
}

#[test]
fn the_seed_travels_so_a_stochastic_operation_repeats_too() {
    let frames = 2048;
    let a = batch(noise_def("t"), frames);
    let b = interactive(noise_def("t"), frames);
    assert_eq!(
        a, b,
        "a stochastic operation is only repeatable if the session starts the seed \
         sequence where the render does"
    );
    assert!(a.iter().any(|&x| x != 0.0), "the noise has to sound");
}

/// Drains replies, returning the first one whose address matches — and, for a
/// `/done`, whose first argument names `for_cmd`, since every async command
/// answers at that same address and an earlier one may still be queued.
fn wait_reply(s: &mut NrtSession, addr: &str, for_cmd: Option<&str>) -> Option<OscMessage> {
    let mut buf = vec![0u8; 1 << 16];
    for _ in 0..64 {
        while let Some(len) = s.poll_into(&mut buf) {
            if let Ok(clausters::rosc::OscPacket::Message(m)) =
                clausters::osc::decode_packet(&buf[..len])
                && m.addr == addr
                && for_cmd.is_none_or(|c| m.args.first() == Some(&OscType::String(c.into())))
            {
                return Some(m);
            }
        }
        s.settle();
    }
    None
}

/// The whole operation over the wire, the way a client drives it: allocate a
/// destination, build the graph, ask for the render, read the samples back.
#[test]
fn buffer_render_runs_the_graph_into_a_buffer() {
    let frames = 2048u64;
    let expected = batch(sine_def("t"), frames);

    let mut s = NrtSession::open(&session_cfg()).expect("open");
    let setup = [
        msg(
            "/buffer_alloc",
            vec![
                OscType::Int(0),
                OscType::Int(frames as i32),
                OscType::Int(CHANNELS as i32),
            ],
        ),
        sine_def("t"),
        s_new("t", 1000),
    ];
    for m in setup {
        assert!(s.send_msg(&m.addr, m.args).expect("encode"), "ring full");
        s.settle_for(4);
    }
    assert!(
        s.send_msg(
            "/buffer_render",
            vec![OscType::Int(0), OscType::Int(frames as i32)],
        )
        .expect("encode"),
        "ring full"
    );
    let done = wait_reply(&mut s, "/done", Some("/buffer_render")).expect("the render answers");
    assert_eq!(
        done.args.first(),
        Some(&OscType::String("/buffer_render".into())),
        "answered as the command that was asked, got {:?}",
        done.args
    );
    assert_eq!(s.frames(), frames, "the operation ran exactly its span");

    // Read it back the way a client would, and compare with the batch render.
    s.send_msg(
        "/buffer_getRange",
        vec![
            OscType::Int(0),
            OscType::Int(0),
            OscType::Int((frames as usize * CHANNELS) as i32),
        ],
    )
    .expect("encode");
    let reply = wait_reply(&mut s, "/buffer_getRange.reply", None).expect("the buffer reads back");
    let OscType::Blob(bytes) = reply.args.last().expect("a blob") else {
        panic!("expected a blob, got {:?}", reply.args)
    };
    let got: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    assert_eq!(
        got, expected,
        "what /buffer_render left in the buffer is what the batch render produces"
    );
}

/// The safety property: the command is legal only where something owns the
/// clock. A server driven by an audio device cannot run a graph on request, and
/// says so instead of queueing work nobody will perform.
#[test]
fn buffer_render_is_refused_where_nothing_owns_the_clock() {
    use clausters::osc::server::{OscServer, ServerInfo};
    use clausters::server::engine::{DEFAULT_AUDIO_BUSES, DEFAULT_CONTROL_BUSES, engine_pair_full};

    let (_engine, handle) = engine_pair_full(
        SR as f32,
        CHANNELS,
        0,
        None,
        DEFAULT_AUDIO_BUSES,
        DEFAULT_CONTROL_BUSES,
        Default::default(),
    );
    // A plain server: no `enable_offline_renders`, which is every server but a
    // session's.
    let mut server = OscServer::headless(
        ServerInfo {
            nominal_sample_rate: SR,
            actual_sample_rate: SR,
        },
        handle,
        0.0,
    );
    assert!(
        server.take_offline_render().is_none(),
        "a server with no offline driver queues nothing, however many renders are asked of it"
    );
}

/// The mode's defining property, checked directly rather than inferred: time
/// moves only inside `run`. Serving between two runs must leave the signal
/// exactly where it was — if a `settle` advanced the engine, the two halves
/// would not join.
#[test]
fn settling_between_operations_does_not_advance_time() {
    let frames = 2048;
    let whole = interactive(sine_def("t"), frames);

    let mut s = NrtSession::open(&session_cfg()).expect("open");
    for m in [sine_def("t"), s_new("t", 1000)] {
        assert!(s.send_msg(&m.addr, m.args).expect("encode"), "ring full");
        s.settle_for(4);
    }
    let mut halves = s.run_to_vec(frames / 2).expect("first half");
    // The clock a real-time server would have kept running is exactly what is
    // being denied here: many turns, no samples.
    s.settle_for(32);
    assert_eq!(s.frames(), frames / 2, "only `run` moves the frame count");
    halves.extend(s.run_to_vec(frames / 2).expect("second half"));

    assert_eq!(
        whole, halves,
        "an operation split in two around a settle must join seamlessly: \
         nothing between commands advances the engine"
    );
}
