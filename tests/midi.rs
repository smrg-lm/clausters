//! M17: standard channel-voice MIDI actuates nodes and their input controls.
//! The central claim: a MIDI-driven voice is **byte-identical** to the
//! equivalent OSC one — `translate_midi` synthesizes the same `/s_new`/
//! `/n_set`/`/n_free` the OSC path would, so the mirrored node state matches.

#![cfg(feature = "synth")]

use clausters::midi::{ChannelVoiceMessage::*, convert};
use clausters::osc::translate::CmdTranslator;
use clausters::rosc::{OscMessage, OscType};
use clausters::server::engine::Cmd;
use clausters_core::registry::NodeIdPartition;

const SR: f32 = 48_000.0;

fn msg(addr: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage {
        addr: addr.into(),
        args,
    }
}

/// Bind channel 0 to the built-in `default` and feed one note-on.
fn bind_and_note(t: &mut CmdTranslator, note: u8, velocity: u16) -> (i32, Vec<Cmd>) {
    let mut cmds = Vec::new();
    t.translate(
        &msg(
            "/midi_bind",
            vec![OscType::Int(0), OscType::String("default".into())],
        ),
        &mut cmds,
    )
    .unwrap();
    cmds.clear();
    t.translate_midi(
        NoteOn {
            channel: 0,
            note,
            velocity,
        },
        &mut cmds,
    )
    .unwrap();
    let id = match cmds.first() {
        Some(Cmd::AddSynth { id, .. }) => *id,
        _ => panic!("expected AddSynth as the first command"),
    };
    (id, cmds)
}

#[test]
fn note_on_spawns_voice_with_converted_controls() {
    let mut t = CmdTranslator::new(SR);
    let (id, _) = bind_and_note(&mut t, 69, u16::MAX);
    // Voice IDs come from the MIDI range of the node-id partition (the
    // default translator uses the default node-table size).
    let part = NodeIdPartition::from_max_nodes(1024);
    assert!(id as i64 >= part.midi_base);
    assert!((id as i64) < part.midi_base + part.midi_capacity as i64);
    let (def_name, controls) = t.mirror.synth_info(id).expect("voice mirrored");
    assert_eq!(def_name, "default");
    let def = t.node_defs.get(&id).unwrap();
    let fi = def.control_index("freq").unwrap() as usize;
    let ai = def.control_index("amp").unwrap() as usize;
    assert!((controls[fi] - 440.0).abs() < 1e-2); // note 69 = A4
    assert!((controls[ai] - 1.0).abs() < 1e-3); // full velocity
}

#[test]
fn midi_voice_matches_equivalent_osc() {
    // MIDI path.
    let mut midi = CmdTranslator::new(SR);
    let (midi_id, _) = bind_and_note(&mut midi, 60, 32768);
    let (_, midi_controls) = midi.mirror.synth_info(midi_id).unwrap();

    // The hand-written OSC the MIDI note is supposed to be equal to.
    let mut osc = CmdTranslator::new(SR);
    let mut cmds = Vec::new();
    let freq = convert::midi2freq(60.0);
    let amp = convert::velocity2amp(32768);
    osc.translate(
        &msg(
            "/s_new",
            vec![
                OscType::String("default".into()),
                OscType::Int(1000),
                OscType::Int(0),
                OscType::Int(0),
                OscType::String("freq".into()),
                OscType::Float(freq),
                OscType::String("amp".into()),
                OscType::Float(amp),
            ],
        ),
        &mut cmds,
    )
    .unwrap();
    let (_, osc_controls) = osc.mirror.synth_info(1000).unwrap();
    assert_eq!(midi_controls, osc_controls);
}

#[test]
fn note_off_frees_the_right_voice() {
    let mut t = CmdTranslator::new(SR);
    let (id, _) = bind_and_note(&mut t, 64, 20000);
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
    assert!(matches!(cmds.as_slice(), [Cmd::FreeNode { id: f }] if *f == id));
    assert!(t.mirror.synth_info(id).is_none());
}

#[test]
fn control_change_sets_mapped_control_on_live_voices() {
    let mut t = CmdTranslator::new(SR);
    let (id, _) = bind_and_note(&mut t, 69, 40000);
    // Route CC 7 to the amp control.
    let mut cmds = Vec::new();
    t.translate(
        &msg(
            "/midi_map",
            vec![
                OscType::Int(0),
                OscType::String("cc7".into()),
                OscType::String("amp".into()),
            ],
        ),
        &mut cmds,
    )
    .unwrap();
    cmds.clear();
    t.translate_midi(
        ControlChange {
            channel: 0,
            controller: 7,
            value: u32::MAX / 4,
        },
        &mut cmds,
    )
    .unwrap();
    let ai = t.node_defs.get(&id).unwrap().control_index("amp").unwrap();
    let want = convert::cc2control(u32::MAX / 4);
    assert!(
        matches!(cmds.as_slice(), [Cmd::SetControl { id: i, index, value }]
            if *i == id && *index == ai && (*value - want).abs() < 1e-4),
        "expected a single SetControl on the voice's amp control",
    );
}

#[test]
fn gate_binding_releases_instead_of_freeing() {
    let mut t = CmdTranslator::new(SR);
    let mut cmds = Vec::new();
    // gate flag = 1.
    t.translate(
        &msg(
            "/midi_bind",
            vec![
                OscType::Int(0),
                OscType::String("default".into()),
                OscType::Int(0),
                OscType::Int(0),
                OscType::Int(1),
            ],
        ),
        &mut cmds,
    )
    .unwrap();
    t.translate_midi(
        NoteOn {
            channel: 0,
            note: 69,
            velocity: 30000,
        },
        &mut cmds,
    )
    .unwrap();
    cmds.clear();
    t.translate_midi(
        NoteOff {
            channel: 0,
            note: 69,
            velocity: 0,
        },
        &mut cmds,
    )
    .unwrap();
    // `default` has no gate control, so the /n_set resolves to a no-op — the
    // point is that no FreeNode is emitted on the gate path.
    assert!(!cmds.iter().any(|c| matches!(c, Cmd::FreeNode { .. })));
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
fn unbind_frees_sounding_voices() {
    let mut t = CmdTranslator::new(SR);
    let (id, _) = bind_and_note(&mut t, 69, 50000);
    let mut cmds = Vec::new();
    t.translate(&msg("/midi_unbind", vec![OscType::Int(0)]), &mut cmds)
        .unwrap();
    assert!(
        cmds.iter()
            .any(|c| matches!(c, Cmd::FreeNode { id: f } if *f == id))
    );
    assert!(t.midi.voices.is_empty());
}

// ---- M19: a persisted binding survives restore and is immediately playable ----

#[test]
fn binding_persists_restores_and_plays() {
    // Bind channel 0 -> default with a cc map, capture the persistable form,
    // restore it into a fresh translator, and confirm a note plays through the
    // default freq/amp map (playable with no further setup).
    let mut a = CmdTranslator::new(SR);
    let mut cmds = Vec::new();
    a.translate(
        &msg(
            "/midi_bind",
            vec![OscType::Int(0), OscType::String("default".into())],
        ),
        &mut cmds,
    )
    .unwrap();
    a.translate(
        &msg(
            "/midi_map",
            vec![
                OscType::Int(0),
                OscType::String("cc1".into()),
                OscType::String("amp".into()),
            ],
        ),
        &mut cmds,
    )
    .unwrap();

    let persisted = a.midi.persist();
    assert_eq!(persisted.len(), 1);

    // A fresh server restores the binding (no OSC re-issued).
    let mut b = CmdTranslator::new(SR);
    let mut bc = Vec::new();
    for pb in persisted {
        b.restore_binding(pb, &mut bc).unwrap();
    }
    let binding = b.midi.channels.get(&0).expect("binding restored");
    assert_eq!(binding.instrument, "default");
    assert_eq!(binding.cc.get(&1).map(String::as_str), Some("amp"));

    // And a note plays immediately: a default voice with freq/amp from the
    // standard conversions.
    bc.clear();
    b.translate_midi(
        NoteOn {
            channel: 0,
            note: 69,
            velocity: 100,
        },
        &mut bc,
    )
    .unwrap();
    let id = *b.midi.voices.get(&(0, 69)).expect("voice spawned");
    let (def_name, controls) = b.mirror.synth_info(id).expect("voice mirrored");
    assert_eq!(def_name, "default");
    let fi = b.node_defs.get(&id).unwrap().control_index("freq").unwrap();
    assert!((controls[fi as usize] - convert::midi2freq(69.0)).abs() < 1e-3);
}
