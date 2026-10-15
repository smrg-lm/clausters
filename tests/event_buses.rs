//! Event buses: notes written on a bus play on whatever reads it.
//!
//! A plugin node's `notes` control names the event bus its main note input
//! reads; `/bus_event` writes there at the sample its bundle names, and
//! `/node_event` gives the same events to one node. The instrument is the
//! polyphonic fixture, whose output is the sum of its held notes'
//! velocities (plus a tenth of their pressure and a hundredth of their
//! tuning) and a constant 0.012, so what it holds can be read off one sample.
#![cfg(all(target_os = "linux", feature = "plugin-clap"))]
use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use clausters::dsp::{Limits, ReplyKind};
use clausters::osc::translate::CmdTranslator;
use clausters::plugin::{PluginDef, PluginReference};
use clausters::rosc::{OscMessage, OscType};
use clausters::server::engine::{Cmd, Engine, EngineHandle, engine_pair_full};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
#[global_allocator]
static ALLOC: AllocDisabler = AllocDisabler;
static FIXTURE: OnceLock<PathBuf> = OnceLock::new();

/// The fixture's output with nothing held but the tempo it adds, 120/10000.
const BASE: f32 = 0.012;

fn fixture() -> &'static PathBuf {
    FIXTURE.get_or_init(|| {
        let dir =
            std::env::temp_dir().join(format!("clausters-event-buses-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("instrument.clap");
        assert!(
            std::process::Command::new("cc")
                .args([
                    "-std=c11",
                    "-O2",
                    "-fPIC",
                    "-shared",
                    "-Ithird_party/clap/include",
                    "crates/clausters-plugin-host/tests/fixtures/polyphonic.c",
                    "-o"
                ])
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        path
    })
}

fn translator() -> CmdTranslator {
    let reference: PluginReference = serde_json::from_value(json!({
        "format": "clap", "path": fixture(), "id": "org.clausters.fixture.instrument",
        "inputs": [], "outputs": [{"id": 0, "channels": 1, "bus": 0}]}))
    .unwrap();
    let mut translator = CmdTranslator::new(48000.0);
    translator.plugin_defs.insert(
        "keys".into(),
        Arc::new(PluginDef::probe(reference, 48000.0).unwrap()),
    );
    translator
}

fn engine() -> (Engine, EngineHandle) {
    engine_pair_full(
        48000.0,
        2,
        0,
        None,
        16,
        16,
        Limits {
            max_nodes: 32,
            ..Limits::default()
        },
    )
}

fn msg(addr: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage {
        addr: addr.into(),
        args,
    }
}

fn translate(t: &mut CmdTranslator, m: OscMessage) -> Vec<Cmd> {
    let mut cmds = Vec::new();
    t.translate(&m, &mut cmds).unwrap();
    cmds
}

fn bus_event(bus: i32, events: Value) -> OscMessage {
    msg(
        "/bus_event",
        vec![OscType::Int(bus), OscType::String(events.to_string())],
    )
}

fn node_event(node: i32, events: Value) -> OscMessage {
    msg(
        "/node_event",
        vec![OscType::Int(node), OscType::String(events.to_string())],
    )
}

/// A `keys` node `id` whose notes come from event bus `bus`.
fn keys(t: &mut CmdTranslator, id: i32, bus: i32) -> Vec<Cmd> {
    translate(
        t,
        msg(
            "/synth_new",
            vec![
                OscType::String("keys".into()),
                OscType::Int(id),
                OscType::Int(1),
                OscType::Int(0),
                OscType::String("notes".into()),
                OscType::Float(bus as f32),
            ],
        ),
    )
}

fn send(handle: &mut EngineHandle, cmds: Vec<Cmd>) {
    for cmd in cmds {
        assert!(handle.send(cmd).is_ok());
    }
}

fn at(handle: &mut EngineHandle, time: u64, cmds: Vec<Cmd>) {
    assert!(handle.send(Cmd::Schedule { time, cmds }).is_ok());
}

fn blocks(engine: &mut Engine, handle: &mut EngineHandle, count: usize) -> Vec<f32> {
    let mut samples = Vec::new();
    for _ in 0..count {
        let mut out = [0.0; 128];
        assert_no_alloc(|| engine.process_block(&mut out));
        samples.extend(out.as_chunks::<2>().0.iter().map(|f| f[0]));
        std::thread::sleep(Duration::from_millis(3));
        while let Some(m) = handle.pop_reply() {
            assert_ne!(m.kind, ReplyKind::Fault, "{m:?}");
        }
        while handle.pop_garbage().is_some() {}
    }
    samples
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-5
}

#[test]
fn a_node_plays_what_its_bus_carries_at_the_sample_written() {
    let mut t = translator();
    let (mut engine, mut handle) = engine();
    send(&mut handle, keys(&mut t, 42, 3));
    let on = translate(
        &mut t,
        bus_event(
            3,
            json!({"kind": "note_on", "key": 60, "note_id": 1, "velocity": 0.5}),
        ),
    );
    assert!(matches!(on[..], [Cmd::BusEvent { bus: 3, .. }]));
    at(&mut handle, 64 * 4 + 17, on);
    // Another bus: nobody reads it.
    let elsewhere = translate(
        &mut t,
        bus_event(4, json!({"kind": "note_on", "key": 62, "velocity": 0.25})),
    );
    at(&mut handle, 64 * 4 + 20, elsewhere);
    let off = translate(
        &mut t,
        bus_event(3, json!({"kind": "note_off", "note_id": 1})),
    );
    at(&mut handle, 64 * 4 + 40, off);
    // The pipeline delays the output by its depth; find the attack.
    let samples = blocks(&mut engine, &mut handle, 12);
    let start = samples
        .iter()
        .position(|v| *v != 0.0)
        .expect("the note sounds");
    assert_eq!(
        start % 64,
        17,
        "the note starts on the sample its bundle names"
    );
    for v in &samples[start..start + 23] {
        assert!(close(*v, 0.5 + BASE), "{v}");
    }
    assert!(samples[start + 23..].iter().all(|v| *v == 0.0));
}

#[test]
fn the_notes_control_moves_a_node_to_another_bus() {
    let mut t = translator();
    let (mut engine, mut handle) = engine();
    send(&mut handle, keys(&mut t, 42, 3));
    send(
        &mut handle,
        translate(
            &mut t,
            msg(
                "/node_set",
                vec![
                    OscType::Int(42),
                    OscType::String("notes".into()),
                    OscType::Float(5.0),
                ],
            ),
        ),
    );
    send(
        &mut handle,
        translate(
            &mut t,
            bus_event(3, json!({"kind": "note_on", "key": 60, "velocity": 0.5})),
        ),
    );
    send(
        &mut handle,
        translate(
            &mut t,
            bus_event(5, json!({"kind": "note_on", "key": 64, "velocity": 0.25})),
        ),
    );
    let samples = blocks(&mut engine, &mut handle, 10);
    assert!(close(*samples.last().unwrap(), 0.25 + BASE));
}

#[test]
fn a_node_event_plays_as_one_read_from_the_bus_and_sources_own_their_notes() {
    let mut t = translator();
    let (mut engine, mut handle) = engine();
    send(&mut handle, keys(&mut t, 42, -1));
    // Two writers, one note id each: an id is its writer's.
    send(
        &mut handle,
        translate(
            &mut t,
            node_event(
                42,
                json!([
                    {"kind": "note_on", "key": 60, "note_id": 7, "velocity": 0.25, "source": 1},
                    {"kind": "note_on", "key": 60, "note_id": 7, "velocity": 0.5, "source": 2}
                ]),
            ),
        ),
    );
    let both = blocks(&mut engine, &mut handle, 8);
    assert!(close(*both.last().unwrap(), 0.75 + BASE));
    send(
        &mut handle,
        translate(
            &mut t,
            node_event(42, json!({"kind": "note_off", "note_id": 7, "source": 1})),
        ),
    );
    let one = blocks(&mut engine, &mut handle, 8);
    assert!(close(*one.last().unwrap(), 0.5 + BASE));
}

#[test]
fn a_joined_note_retunes_the_held_one_on_a_native_port() {
    let mut t = translator();
    let (mut engine, mut handle) = engine();
    send(&mut handle, keys(&mut t, 42, 2));
    send(
        &mut handle,
        translate(
            &mut t,
            bus_event(
                2,
                json!([
                    {"kind": "note_on", "key": 60, "note_id": 1, "velocity": 0.5},
                    {"kind": "note_on", "key": 64, "note_id": 2, "joins": 1, "velocity": 0.5}
                ]),
            ),
        ),
    );
    // One note, four semitones up: its tuning adds 4 * 0.01.
    let joined = blocks(&mut engine, &mut handle, 8);
    assert!(
        close(*joined.last().unwrap(), 0.5 + 0.04 + BASE),
        "{}",
        joined.last().unwrap()
    );
    send(
        &mut handle,
        translate(
            &mut t,
            bus_event(2, json!({"kind": "note_off", "note_id": 2})),
        ),
    );
    let ended = blocks(&mut engine, &mut handle, 8);
    assert_eq!(
        *ended.last().unwrap(),
        0.0,
        "the line ends with its last note"
    );
}

#[test]
fn channel_messages_reach_the_held_notes_and_what_has_no_spelling_is_dropped() {
    let mut t = translator();
    let (mut engine, mut handle) = engine();
    send(&mut handle, keys(&mut t, 42, 1));
    send(
        &mut handle,
        translate(
            &mut t,
            bus_event(
                1,
                json!([
                    {"kind": "note_on", "key": 60, "velocity": 0.5},
                    {"kind": "pressure", "channel": 0, "value": 1.0},
                    // Controller 21 has no native spelling, the port takes
                    // native notes: dropped, and the instance stays healthy.
                    {"kind": "controller", "channel": 0, "controller": 21, "value": 0.5},
                    {"kind": "program", "channel": 0, "program": 3}
                ]),
            ),
        ),
    );
    let pressed = blocks(&mut engine, &mut handle, 8);
    assert!(
        close(*pressed.last().unwrap(), 0.5 + 0.1 + BASE),
        "{}",
        pressed.last().unwrap()
    );
}

#[test]
fn a_bus_event_is_checked_before_it_is_sent() {
    let mut t = translator();
    let mut cmds = Vec::new();
    let bad = [
        bus_event(64, json!({"kind": "note_on", "key": 60})),
        bus_event(-1, json!({"kind": "note_on", "key": 60})),
        bus_event(
            0,
            json!({"kind": "control", "control": "cutoff", "value": 1}),
        ),
        bus_event(0, json!({"kind": "note_on", "key": 200})),
    ];
    for m in bad {
        assert!(t.translate(&m, &mut cmds).is_err(), "{m:?}");
    }
    send_node(&mut t);
    assert!(
        t.translate(
            &node_event(42, json!({"kind": "control", "control": "x", "value": 1})),
            &mut cmds
        )
        .unwrap_err()
        .contains("/node_set")
    );
    assert!(
        t.translate(
            &node_event(7, json!({"kind": "note_on", "key": 60})),
            &mut cmds
        )
        .is_err()
    );
    assert!(cmds.is_empty());
}

fn send_node(t: &mut CmdTranslator) {
    keys(t, 42, -1);
}
