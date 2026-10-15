//! The `Voices` node: N voices of one def, built with the node and given to
//! notes on the audio thread.
//!
//! The instrument here is a level and a gate: its output is `amp` while the
//! gate is open, falling to zero over 5 ms when it closes, after which it
//! frees itself. So one sample of bus 0 says how many voices sound and at
//! what velocity, and a voice that freed itself is one a note can take again.

#![cfg(feature = "synth")]

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use clausters::dsp::Limits;
use clausters::osc::translate::CmdTranslator;
use clausters::rosc::{OscMessage, OscType};
use clausters::server::engine::{Cmd, Engine, EngineHandle, engine_pair_full};
use serde_json::{Value, json};

#[global_allocator]
static ALLOC: AllocDisabler = AllocDisabler;

const SR: f32 = 48_000.0;

fn msg(addr: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage {
        addr: addr.into(),
        args,
    }
}

fn s(v: &str) -> OscType {
    OscType::String(v.into())
}
fn i(v: i32) -> OscType {
    OscType::Int(v)
}
fn f(v: f32) -> OscType {
    OscType::Float(v)
}

/// `level`: `amp` while `gate` is open, released over 5 ms, then freed.
fn level_def() -> Value {
    let mut env = vec![
        json!({"control": 2}),
        json!({"const": 1.0}),
        json!({"const": 0.0}),
        json!({"const": 1.0}),
        json!({"const": 2.0}),
        json!({"const": 0.0}),
        json!({"const": 2.0}),
        json!({"const": 1.0}),
        json!({"const": -99.0}),
    ];
    for seg in [[1.0, 0.0, 1.0, 0.0], [0.0, 0.005, 1.0, 0.0]] {
        env.extend(seg.iter().map(|v| json!({ "const": v })));
    }
    json!({
        "name": "level",
        "controls": [
            {"name": "freq", "default": 440.0},
            {"name": "amp", "default": 0.1},
            {"name": "gate", "default": 1.0},
            {"name": "out", "default": 0.0}
        ],
        "ugens": [
            {"kind": "EnvGen", "inputs": env},
            {"kind": "Mul", "inputs": [{"ugen": 0}, {"control": 1}]},
            {"kind": "Out", "inputs": [{"control": 3}, {"ugen": 1}]}
        ]
    })
}

fn setup() -> (CmdTranslator, Engine, EngineHandle) {
    let mut t = CmdTranslator::new(SR);
    t.d_recv(&[s(&level_def().to_string())]).unwrap();
    let (engine, handle) = engine_pair_full(
        SR,
        2,
        0,
        None,
        16,
        16,
        Limits {
            max_nodes: 64,
            ..Limits::default()
        },
    );
    (t, engine, handle)
}

fn translate(t: &mut CmdTranslator, m: OscMessage) -> Vec<Cmd> {
    let mut cmds = Vec::new();
    t.translate(&m, &mut cmds).unwrap();
    cmds
}

fn send(handle: &mut EngineHandle, cmds: Vec<Cmd>) {
    for cmd in cmds {
        assert!(handle.send(cmd).is_ok());
    }
}

/// A `Voices` node 100 of `count` voices of `level` reading event bus 2.
fn voices(t: &mut CmdTranslator, count: i32) -> Vec<Cmd> {
    translate(
        t,
        msg(
            "/voices_new",
            vec![s("level"), i(100), i(1), i(0), i(count), s("notes"), f(2.0)],
        ),
    )
}

fn bus_event(events: Value) -> OscMessage {
    msg("/bus_event", vec![i(2), s(&events.to_string())])
}

fn on(key: i32, id: i32, velocity: f64) -> Value {
    json!({"kind": "note_on", "key": key, "note_id": id, "velocity": velocity})
}

fn off(id: i32) -> Value {
    json!({"kind": "note_off", "note_id": id})
}

fn blocks(engine: &mut Engine, handle: &mut EngineHandle, count: usize) -> Vec<f32> {
    let mut samples = Vec::new();
    for _ in 0..count {
        let mut out = [0.0; 128];
        assert_no_alloc(|| engine.process_block(&mut out));
        samples.extend(out.as_chunks::<2>().0.iter().map(|frame| frame[0]));
        while handle.pop_garbage().is_some() {}
    }
    samples
}

fn last(samples: &[f32]) -> f32 {
    *samples.last().unwrap()
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-5
}

#[test]
fn notes_take_voices_and_no_node_is_made_for_one() {
    let (mut t, mut engine, mut handle) = setup();
    let made = voices(&mut t, 4);
    assert_eq!(
        made.iter()
            .filter(|c| matches!(c, Cmd::AddSynth { .. }))
            .count(),
        1,
        "one node, made once"
    );
    send(&mut handle, made);
    let mut notes = Vec::new();
    for m in [
        bus_event(on(60, 1, 0.25)),
        bus_event(on(64, 2, 0.5)),
        bus_event(off(1)),
    ] {
        notes.extend(translate(&mut t, m));
    }
    assert!(
        notes.iter().all(|c| matches!(c, Cmd::BusEvent { .. })),
        "a note on the event path makes no node"
    );
    let rest = notes.split_off(2);
    send(&mut handle, notes);
    assert!(close(last(&blocks(&mut engine, &mut handle, 2)), 0.75));
    send(&mut handle, rest);
    // The released note falls to nothing in 5 ms and frees its voice.
    assert!(close(last(&blocks(&mut engine, &mut handle, 8)), 0.5));
    assert_eq!(t.node_defs.len(), 1);
}

#[test]
fn a_full_node_steals_the_oldest_and_a_freed_voice_is_given_again() {
    let (mut t, mut engine, mut handle) = setup();
    send(&mut handle, voices(&mut t, 2));
    send(
        &mut handle,
        translate(
            &mut t,
            bus_event(json!([on(60, 1, 0.125), on(62, 2, 0.25)])),
        ),
    );
    assert!(close(last(&blocks(&mut engine, &mut handle, 2)), 0.375));
    // A third note: the oldest held voice is stolen.
    send(&mut handle, translate(&mut t, bus_event(on(64, 3, 0.5))));
    assert!(close(last(&blocks(&mut engine, &mut handle, 2)), 0.75));
    // Releasing the stolen note's id ends nothing: that voice plays note 3.
    send(&mut handle, translate(&mut t, bus_event(off(1))));
    assert!(close(last(&blocks(&mut engine, &mut handle, 8)), 0.75));
    send(
        &mut handle,
        translate(&mut t, bus_event(json!([off(2), off(3)]))),
    );
    assert_eq!(last(&blocks(&mut engine, &mut handle, 8)), 0.0);
    // Both voices freed themselves: two notes sound again, at once.
    send(
        &mut handle,
        translate(
            &mut t,
            bus_event(json!([on(60, 4, 0.125), on(62, 5, 0.25)])),
        ),
    );
    assert!(close(last(&blocks(&mut engine, &mut handle, 2)), 0.375));
}

#[test]
fn a_set_on_the_node_reaches_every_voice_and_a_note_control_one() {
    let (mut t, mut engine, mut handle) = setup();
    send(&mut handle, voices(&mut t, 4));
    send(
        &mut handle,
        translate(&mut t, bus_event(json!([on(60, 1, 0.25), on(62, 2, 0.25)]))),
    );
    blocks(&mut engine, &mut handle, 2);
    // A note's own control, by name, on its voice alone.
    send(
        &mut handle,
        translate(
            &mut t,
            msg(
                "/node_event",
                vec![
                    i(100),
                    s(
                        &json!({"kind": "control", "note_id": 2, "control": "amp", "value": 0.5})
                            .to_string(),
                    ),
                ],
            ),
        ),
    );
    assert!(close(last(&blocks(&mut engine, &mut handle, 2)), 0.75));
    // The node's `out`: every voice moves to bus 1.
    send(
        &mut handle,
        translate(&mut t, msg("/node_set", vec![i(100), s("out"), f(1.0)])),
    );
    assert_eq!(last(&blocks(&mut engine, &mut handle, 2)), 0.0);
}

#[test]
fn the_voice_map_names_what_a_note_sets() {
    let (mut t, mut engine, mut handle) = setup();
    send(&mut handle, voices(&mut t, 2));
    // Velocity to nothing: every note sounds at the node's own `amp`.
    send(
        &mut handle,
        translate(&mut t, msg("/voices_map", vec![i(100), s("vel"), s("")])),
    );
    send(
        &mut handle,
        translate(&mut t, msg("/node_set", vec![i(100), s("amp"), f(0.3)])),
    );
    send(&mut handle, translate(&mut t, bus_event(on(60, 1, 1.0))));
    assert!(close(last(&blocks(&mut engine, &mut handle, 2)), 0.3));
    // A controller to `amp`: the node's, for the held voices and the next.
    send(
        &mut handle,
        translate(&mut t, msg("/voices_map", vec![i(100), s("cc7"), s("amp")])),
    );
    send(
        &mut handle,
        translate(
            &mut t,
            bus_event(json!({"kind": "controller", "channel": 0, "controller": 7, "value": 0.5})),
        ),
    );
    assert!(close(last(&blocks(&mut engine, &mut handle, 2)), 0.5));
    let mut cmds = Vec::new();
    assert!(
        t.translate(
            &msg("/voices_map", vec![i(100), s("wobble"), s("amp")]),
            &mut cmds
        )
        .is_err()
    );
    assert!(
        t.translate(
            &msg("/voices_map", vec![i(100), s("vel"), s("nothing")]),
            &mut cmds
        )
        .is_err()
    );
}

#[test]
fn a_joined_note_keeps_its_voice() {
    let (mut t, mut engine, mut handle) = setup();
    send(&mut handle, voices(&mut t, 4));
    send(&mut handle, translate(&mut t, bus_event(on(60, 1, 0.25))));
    blocks(&mut engine, &mut handle, 2);
    send(
        &mut handle,
        translate(
            &mut t,
            bus_event(
                json!({"kind": "note_on", "key": 64, "note_id": 2, "joins": 1, "velocity": 0.5}),
            ),
        ),
    );
    // One voice, at the joined note's velocity: the line did not restart.
    assert!(close(last(&blocks(&mut engine, &mut handle, 2)), 0.5));
    send(&mut handle, translate(&mut t, bus_event(off(2))));
    assert_eq!(last(&blocks(&mut engine, &mut handle, 8)), 0.0);
}

#[test]
fn a_voices_node_is_refused_what_cannot_be_a_voice() {
    let (mut t, _, _) = setup();
    let mut cmds = Vec::new();
    for args in [
        vec![s("level"), i(100), i(1), i(0), i(0)],
        vec![s("level"), i(100), i(1), i(0), i(1000)],
        vec![s("missing"), i(100), i(1), i(0), i(2)],
    ] {
        assert!(t.translate(&msg("/voices_new", args), &mut cmds).is_err());
    }
    assert!(cmds.is_empty());
}

/// `tone`: a sine at `freq`, `amp` under a gate released over 5 ms.
fn tone_def() -> Value {
    let mut def = level_def();
    def["name"] = json!("tone");
    def["ugens"] = json!([
        def["ugens"][0].clone(),
        {"kind": "Sine", "inputs": [{"control": 0}]},
        {"kind": "Mul", "inputs": [{"ugen": 0}, {"ugen": 1}]},
        {"kind": "Mul", "inputs": [{"ugen": 2}, {"control": 1}]},
        {"kind": "Out", "inputs": [{"control": 3}, {"ugen": 3}]}
    ]);
    def
}

#[test]
fn a_voices_node_renders_offline_as_it_plays_live() {
    use clausters::server::render::{OscScore, RenderConfig, render_to_vec};
    let def = msg("/def_send", vec![s("synth"), s(&tone_def().to_string())]);
    let make = msg(
        "/voices_new",
        vec![s("tone"), i(100), i(1), i(0), i(3), s("notes"), f(2.0)],
    );
    // Notes on and off at samples inside blocks, a steal, a joined note and a
    // voice freed and given again.
    let events: Vec<(u64, Value)> = vec![
        (70, on(60, 1, 0.25)),
        (90, on(64, 2, 0.25)),
        (200, on(67, 3, 0.25)),
        (333, on(72, 4, 0.25)),
        (
            500,
            json!({"kind": "note_on", "key": 74, "note_id": 5, "joins": 4, "velocity": 0.3}),
        ),
        (700, off(2)),
        (900, off(3)),
        (1300, on(55, 6, 0.4)),
        (1500, json!([off(5), off(6)])),
    ];
    let mut t = CmdTranslator::new(SR);
    let (mut engine, mut handle) = engine_pair_full(SR, 2, 0, None, 16, 16, Limits::default());
    t.d_recv(&def.args[1..]).unwrap();
    send(&mut handle, translate(&mut t, make.clone()));
    for (at, e) in &events {
        let cmds = translate(&mut t, bus_event(e.clone()));
        assert!(handle.send(Cmd::Schedule { time: *at, cmds }).is_ok());
    }
    let mut live = Vec::new();
    for _ in 0..40 {
        let mut block = [0.0; 128];
        assert_no_alloc(|| engine.process_block(&mut block));
        live.extend(block);
        while handle.pop_garbage().is_some() {}
    }
    let mut score = vec![(0.0, vec![def, make])];
    score.extend(
        events
            .into_iter()
            .map(|(at, e)| (at as f64 / f64::from(SR), vec![bus_event(e)])),
    );
    score.push((
        40.0 * 64.0 / f64::from(SR),
        vec![msg("/node_free", vec![i(100)])],
    ));
    let cfg = RenderConfig {
        channels: 2,
        seed: Some(1),
        ..Default::default()
    };
    let offline = render_to_vec(&OscScore::new(score).unwrap(), &cfg)
        .unwrap()
        .0;
    assert!(live.iter().any(|v| *v != 0.0));
    assert_eq!(live, offline[..live.len()]);
}
