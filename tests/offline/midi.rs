//! Standard channel-voice MIDI is routing: a channel writes an event bus, and
//! whatever reads the bus plays it. The instrument form of `/midi_bind` makes
//! that reader -- a `Voices` node of the def -- on a bus of the binding's, so
//! a note makes no node. The central claim, kept from when a note was a
//! synth: a MIDI note is **the same event** a client's `/bus_event` writes.

#![cfg(feature = "synth")]

use clausters::dsp::Limits;
use clausters::midi::ChannelVoiceMessage::*;
use clausters::osc::translate::CmdTranslator;
use clausters::rosc::{OscMessage, OscType};
use clausters::server::engine::{Cmd, engine_pair_full};
use clausters_core::event_bus::{BusEvent, Kind, midi_source};
use clausters_core::registry::NodeIdPartition;

const SR: f32 = 48_000.0;

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

/// Binds channel 0 to the built-in `default`: the node it made and the bus
/// the channel writes.
fn bind(t: &mut CmdTranslator) -> (i32, u32, Vec<Cmd>) {
    let cmds = translate(
        t,
        msg(
            "/midi_bind",
            vec![OscType::Int(0), OscType::String("default".into())],
        ),
    );
    let b = &t.midi.channels[&0];
    (b.node.expect("a node"), b.bus.expect("a bus"), cmds)
}

fn events(cmds: &[Cmd]) -> Vec<(u32, BusEvent)> {
    cmds.iter()
        .filter_map(|c| match c {
            Cmd::BusEvent { bus, event } => Some((*bus, *event)),
            _ => None,
        })
        .collect()
}

fn note(t: &mut CmdTranslator, key: u8, velocity: u16) -> Vec<Cmd> {
    let mut cmds = Vec::new();
    t.translate_midi(
        NoteOn {
            channel: 0,
            note: key,
            velocity,
        },
        &mut cmds,
    )
    .unwrap();
    cmds
}

#[test]
fn an_instrument_binding_makes_one_voices_node_and_a_note_makes_none() {
    let mut t = CmdTranslator::new(SR);
    let (node, bus, made) = bind(&mut t);
    assert_eq!(
        made.iter()
            .filter(|c| matches!(c, Cmd::AddSynth { .. }))
            .count(),
        1
    );
    let part = NodeIdPartition::from_max_nodes(Limits::default().max_nodes);
    assert!((part.midi_base..part.midi_base + part.midi_capacity as i64).contains(&(node as i64)));
    let (def_name, controls) = t.mirror.synth_info(node).expect("node mirrored");
    assert_eq!(def_name, "default");
    let def = t.node_defs.get(&node).unwrap();
    assert!(def.reads_notes());
    let notes = def.control_index("notes").unwrap() as usize;
    assert_eq!(
        controls[notes], bus as f32,
        "the node reads the binding's bus"
    );

    let cmds = note(&mut t, 69, u16::MAX);
    assert!(!cmds.iter().any(|c| matches!(c, Cmd::AddSynth { .. })));
    let [(on_bus, on)] = events(&cmds)[..] else {
        panic!("one event: {}", cmds.len());
    };
    assert_eq!(on_bus, bus);
    assert_eq!(
        (on.kind, on.channel, on.key, on.note_id),
        (Kind::NoteOn, 0, 69, -1)
    );
    assert_eq!(on.value, 1.0);
    assert_eq!(on.source, midi_source(0));
}

#[test]
fn a_midi_note_is_the_event_a_client_writes() {
    let mut t = CmdTranslator::new(SR);
    let (_, bus, _) = bind(&mut t);
    let midi = events(&note(&mut t, 60, 32768))[0].1;
    let written = translate(
        &mut t,
        msg(
            "/bus_event",
            vec![
                OscType::Int(bus as i32),
                OscType::String(
                    serde_json::json!({"kind": "note_on", "channel": 0, "key": 60,
                                       "velocity": 32768.0 / 65535.0})
                    .to_string(),
                ),
            ],
        ),
    );
    let client = events(&written)[0].1;
    // Only the writer differs.
    assert_eq!(
        BusEvent { source: 0, ..midi },
        BusEvent {
            source: 0,
            ..client
        }
    );
}

#[test]
fn a_note_off_and_a_controller_are_events_on_the_bus() {
    let mut t = CmdTranslator::new(SR);
    let (_, bus, _) = bind(&mut t);
    let mut cmds = Vec::new();
    t.translate_midi(
        NoteOff {
            channel: 0,
            note: 64,
            velocity: 0,
        },
        &mut cmds,
    )
    .unwrap();
    t.translate_midi(
        ControlChange {
            channel: 0,
            controller: 7,
            value: u32::MAX / 4,
        },
        &mut cmds,
    )
    .unwrap();
    let got = events(&cmds);
    assert_eq!(got.len(), 2);
    assert_eq!(
        (got[0].0, got[0].1.kind, got[0].1.key),
        (bus, Kind::NoteOff, 64)
    );
    assert_eq!((got[1].1.kind, got[1].1.index), (Kind::Controller, 7));
    assert!((got[1].1.value - 0.25).abs() < 1e-6);
}

#[test]
fn a_midi_map_is_the_nodes_voice_map() {
    let mut t = CmdTranslator::new(SR);
    let (node, _, _) = bind(&mut t);
    let cmds = translate(
        &mut t,
        msg(
            "/midi_map",
            vec![
                OscType::Int(0),
                OscType::String("cc7".into()),
                OscType::String("amp".into()),
            ],
        ),
    );
    let amp = t.node_defs[&node].control_index("amp");
    assert!(
        matches!(cmds.as_slice(), [Cmd::VoiceMap { id, index, .. }] if *id == node && *index == amp),
        "one voice-map entry on the node"
    );
    let mut cmds = Vec::new();
    assert!(
        t.translate(
            &msg(
                "/midi_map",
                vec![
                    OscType::Int(0),
                    OscType::String("prog3".into()),
                    OscType::String("other".into()),
                ],
            ),
            &mut cmds
        )
        .is_err()
    );
}

#[test]
fn a_channel_routed_to_a_bus_writes_there_and_makes_nothing() {
    let mut t = CmdTranslator::new(SR);
    let made = translate(
        &mut t,
        msg("/midi_bind", vec![OscType::Int(3), OscType::Int(5)]),
    );
    assert!(made.is_empty());
    let mut cmds = Vec::new();
    t.translate_midi(
        NoteOn {
            channel: 3,
            note: 60,
            velocity: 1000,
        },
        &mut cmds,
    )
    .unwrap();
    assert!(matches!(events(&cmds)[..], [(5, e)] if e.channel == 3 && e.source == midi_source(3)));
    assert!(
        t.translate(
            &msg(
                "/midi_map",
                vec![
                    OscType::Int(3),
                    OscType::String("cc7".into()),
                    OscType::String("amp".into()),
                ],
            ),
            &mut cmds
        )
        .unwrap_err()
        .contains("/voices_map")
    );
    assert!(
        t.translate(
            &msg("/midi_bind", vec![OscType::Int(3), OscType::Int(64)]),
            &mut cmds
        )
        .is_err(),
        "a bus out of range"
    );
}

#[test]
fn unbound_channel_is_silently_ignored() {
    let mut t = CmdTranslator::new(SR);
    let mut cmds = Vec::new();
    t.translate_midi(
        NoteOn {
            channel: 5,
            note: 69,
            velocity: 1000,
        },
        &mut cmds,
    )
    .unwrap();
    assert!(cmds.is_empty());
}

#[test]
fn unbind_frees_the_node_the_binding_made() {
    let mut t = CmdTranslator::new(SR);
    let (node, _, _) = bind(&mut t);
    let cmds = translate(&mut t, msg("/midi_unbind", vec![OscType::Int(0)]));
    assert!(
        cmds.iter()
            .any(|c| matches!(c, Cmd::FreeNode { id } if *id == node))
    );
    assert!(t.midi.channels.is_empty());
}

#[test]
fn a_binding_persists_and_restores_its_node_and_its_map() {
    let mut a = CmdTranslator::new(SR);
    bind(&mut a);
    translate(
        &mut a,
        msg(
            "/midi_map",
            vec![
                OscType::Int(0),
                OscType::String("cc1".into()),
                OscType::String("amp".into()),
            ],
        ),
    );
    let persisted = a.midi.persist();
    assert_eq!(persisted.len(), 1);

    let mut b = CmdTranslator::new(SR);
    let mut cmds = Vec::new();
    for pb in persisted {
        b.restore_binding(pb, &mut cmds).unwrap();
    }
    let binding = &b.midi.channels[&0];
    assert_eq!(binding.instrument, "default");
    assert_eq!(binding.cc.get(&1).map(String::as_str), Some("amp"));
    let node = binding.node.expect("the node made again");
    let bus = binding.bus.unwrap();
    let amp = b.node_defs[&node].control_index("amp");
    assert!(
        cmds.iter()
            .any(|c| matches!(c, Cmd::AddSynth { id, .. } if *id == node))
    );
    assert!(
        cmds.iter()
            .any(|c| matches!(c, Cmd::VoiceMap { id, index, .. }
            if *id == node && *index == amp)),
        "the map travels with the binding"
    );
    assert_eq!(events(&note(&mut b, 69, 100))[0].0, bus);
}

#[test]
fn a_bound_channel_sounds_through_its_node() {
    let mut t = CmdTranslator::new(SR);
    let (mut engine, mut handle) = engine_pair_full(SR, 2, 0, None, 16, 16, Limits::default());
    let (_, _, made) = bind(&mut t);
    for cmd in made.into_iter().chain(note(&mut t, 69, 40000)) {
        assert!(handle.send(cmd).is_ok());
    }
    let mut heard = 0.0f32;
    for _ in 0..40 {
        let mut out = [0.0; 128];
        engine.process_block(&mut out);
        heard = out.iter().fold(heard, |m, v| m.max(v.abs()));
        while handle.pop_garbage().is_some() {}
    }
    assert!(heard > 0.01, "the default voice sounds: {heard}");
}
