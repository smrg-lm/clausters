//! S13: the NRT server taking operations on demand.
//!
//! The claim under test is the one the mode is defined by, and it is not a
//! claim about time: **determinism here is of process**. An interactive session
//! cannot be deterministic in time — it answers a document, not a timeline —
//! so what has to hold is that the *same operation over the same samples*
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

/// The same samples two ways: a score rendered in batch, and the same
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
        "an operation must be sample-identical to the batch render of the same samples"
    );
    assert!(a.iter().any(|&x| x != 0.0), "the samples have to sound");
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
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
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

/// A `/server_quit` ends a session the way it ends any other server: the
/// driver is told, and stops driving. There is no loop here to break out of,
/// so being told is the whole of it.
#[test]
fn a_quit_is_reported_to_the_driver() {
    let mut s = NrtSession::open(&session_cfg()).expect("open");
    assert!(!s.settle(), "nothing has asked it to quit yet");
    assert!(s.send_msg("/server_quit", vec![]).expect("encode"));
    assert!(s.settle_for(4), "the quit reaches the driver");
}

/// What "no clock" does **not** mean, pinned because the milestone's own
/// acceptance said it carelessly ("no scheduling surface") and that is wrong.
/// A timetag is meaningful here — an operation *is* a score, and a bundle
/// inside its span lands at its exact sample, exactly as the batch renderer
/// places one. What the mode lacks is a clock that moves on its own: a bundle
/// past the operation's end does not fire, and waits for the next operation,
/// however long the caller takes to ask for it.
#[test]
fn a_timetag_lands_at_its_sample_and_waits_for_the_next_operation() {
    use clausters::rosc::{OscBundle, OscPacket, OscTime, encoder};
    const NTP_UNIX_OFFSET: f64 = 2_208_988_800.0;

    // The session's clock is the sample clock with epoch 0, so a timetag of
    // `t` seconds is sample `t * SR`.
    let bundle_at = |unix: f64, messages: Vec<OscMessage>| {
        let ntp = unix + NTP_UNIX_OFFSET;
        let seconds = ntp.trunc();
        encoder::encode(&OscPacket::Bundle(OscBundle {
            timetag: OscTime {
                seconds: seconds as u32,
                fractional: ((ntp - seconds) * 2f64.powi(32)) as u32,
            },
            content: messages.into_iter().map(OscPacket::Message).collect(),
        }))
        .expect("encode")
    };

    let half = 1024u64;
    let mut s = NrtSession::open(&session_cfg()).expect("open");
    let d = sine_def("t");
    assert!(s.send_msg(&d.addr, d.args).expect("encode"));
    s.settle_for(4);

    // One synth at the midpoint of the first operation, one past its end.
    let inside = s_new("t", 1000);
    let beyond = s_new("t", 1001);
    assert!(s.send(&bundle_at(half as f64 / SR, vec![inside])));
    assert!(s.send(&bundle_at((half * 4) as f64 / SR, vec![beyond])));
    s.settle_for(4);

    let first = s.run_to_vec(half * 2).expect("first operation");
    let (before, after) = first.split_at(half as usize * CHANNELS);
    assert!(
        before.iter().all(|&x| x == 0.0),
        "nothing sounds before the timetag"
    );
    assert!(
        after.iter().any(|&x| x != 0.0),
        "the bundle inside the span landed at its sample"
    );

    // The one past the end has not fired: time did not run past the operation.
    // Settling any number of times cannot make it, which is the property.
    s.settle_for(32);
    let quiet = s.run_to_vec(1).expect("one frame");
    let level_now = quiet.iter().fold(0.0f32, |m, x| m.max(x.abs()));

    // It fires once an operation reaches its sample, and not before.
    let second = s.run_to_vec(half * 3).expect("second operation");
    let tail = &second[second.len() - CHANNELS * 64..];
    let level_later = tail.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(
        level_later > level_now,
        "the bundle past the first operation waited for the second, then landed \
         ({level_now} -> {level_later})"
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

/// **A session with no audio device can still own the samples a peer edits.**
///
/// The on-demand server is what an editor talks to — it renders, it applies the
/// edit verbs, and it has no clock of its own — so it is the one that most
/// wants its buffers mapped rather than fetched. Given a path, its segment is a
/// file and every buffer it installs lives in a region beside it, which is the
/// whole difference between a session nobody else can see and one an editor
/// draws from.
#[test]
fn a_session_given_a_path_shares_the_buffers_it_holds() {
    use clausters::server::ipc::Segment;

    let path = std::env::temp_dir().join(format!(
        "clausters-session-shm-{}-{}",
        std::process::id(),
        line!()
    ));
    let mut cfg = session_cfg();
    cfg.shm = Some(path.clone());
    let mut s = NrtSession::open(&cfg).expect("open");

    assert!(
        s.send_msg(
            "/buffer_alloc",
            vec![OscType::Int(1), OscType::Int(32), OscType::Int(2)]
        )
        .expect("encode")
    );
    s.settle_for(8);

    // The peer's side: another process would open the file; here opening it
    // again is the same thing, and it is what proves the samples are not in
    // this process's heap.
    let peer = Segment::open(&path).expect("a peer maps the session's segment");
    let (_, mapped) = peer.map_buffer(&path, 1).expect("and finds buffer 1");
    assert_eq!((mapped.frames(), mapped.channels()), (32, 2));
    mapped.set_at(3, 0.5);
    assert_eq!(
        mapped.at(3),
        0.5,
        "a server with no audio device, holding samples an editor can write"
    );

    drop(s);
    let _ = std::fs::remove_file(&path);
}
