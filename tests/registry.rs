//! The finite-resource registries on the server side: the auto node-id range
//! (`/s_new -1`, GraphDef members) and the MIDI voice range are occupancy
//! maps scaled from `--max-nodes`, recycled as nodes die — never counters.
//! Exhaustion is an explicit command error; a failed instantiation hands back
//! every id and bus it took.

#![cfg(feature = "synth")]

use clausters::dsp::Limits;
use clausters::midi::ChannelVoiceMessage::{NoteOff, NoteOn};
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

fn tiny_translator(max_nodes: usize) -> CmdTranslator {
    let limits = Limits {
        max_nodes,
        ..Limits::default()
    };
    CmdTranslator::with_limits(SR, 128, 1024, limits)
}

fn s_new_auto(t: &mut CmdTranslator) -> Result<i32, String> {
    let mut cmds = Vec::new();
    t.translate(
        &msg(
            "/s_new",
            vec![
                OscType::String("default".into()),
                OscType::Int(-1),
                OscType::Int(1),
                OscType::Int(0),
            ],
        ),
        &mut cmds,
    )?;
    match cmds.first() {
        Some(Cmd::AddSynth { id, .. }) => Ok(*id),
        other => panic!("expected AddSynth, got {:?}", other.is_some()),
    }
}

#[test]
fn auto_ids_recycle_on_node_end_and_exhaust_explicitly() {
    // max_nodes 4 -> auto range capacity 8 (2 * max_nodes).
    let mut t = tiny_translator(4);
    let part = NodeIdPartition::from_max_nodes(4);
    let mut ids = Vec::new();
    for _ in 0..part.auto_capacity {
        let id = s_new_auto(&mut t).expect("within capacity");
        assert!((id as i64) >= part.auto_base);
        assert!((id as i64) < part.auto_base + part.auto_capacity as i64);
        ids.push(id);
    }
    // Exhaustion: an explicit error, not a wrapped or out-of-range id.
    let err = s_new_auto(&mut t).unwrap_err();
    assert!(err.contains("out of auto node ids"), "got: {err}");

    // A node death (the engine's End event, fed back by the server loop)
    // returns the id; the range immediately serves again.
    t.release_node_id(ids[0]);
    let reused = s_new_auto(&mut t).expect("released id makes room");
    assert_eq!(reused, ids[0], "the one free id is the one released");

    // Cycling death + spawn forever never exhausts: the registry recycles.
    for _ in 0..100 {
        t.release_node_id(reused);
        assert_eq!(s_new_auto(&mut t).unwrap(), reused);
    }
}

#[test]
fn midi_voice_ids_recycle_with_the_voices() {
    let mut t = tiny_translator(4);
    let part = NodeIdPartition::from_max_nodes(4);
    let mut cmds = Vec::new();
    t.translate(
        &msg(
            "/midi_bind",
            vec![OscType::Int(0), OscType::String("default".into())],
        ),
        &mut cmds,
    )
    .unwrap();

    // Note-on/off cycles far past the range's width: each voice's death
    // (note-off frees the node; its End event releases the id) keeps the
    // range serving.
    for _ in 0..(part.midi_capacity * 3) {
        cmds.clear();
        t.translate_midi(
            NoteOn {
                channel: 0,
                note: 60,
                velocity: 30000,
            },
            &mut cmds,
        )
        .unwrap();
        let id = match cmds.first() {
            Some(Cmd::AddSynth { id, .. }) => *id,
            _ => panic!("expected AddSynth"),
        };
        assert!((id as i64) >= part.midi_base);
        assert!((id as i64) < part.midi_base + part.midi_capacity as i64);
        t.translate_midi(
            NoteOff {
                channel: 0,
                note: 60,
                velocity: 0,
            },
            &mut cmds,
        )
        .unwrap();
        t.release_node_id(id); // the End event the server loop would feed
    }
}

#[test]
fn failed_graph_instantiation_leaks_no_ids() {
    // A GraphDef referencing a missing member def fails at make_synth —
    // after ids were not yet taken; and a def whose second member is missing
    // fails the same way. Either way, repeating the failure far past the
    // auto range's width keeps failing with the *def* error, never with an
    // id-exhaustion error — nothing leaks.
    let mut t = tiny_translator(4);
    let part = NodeIdPartition::from_max_nodes(4);
    t.d_graph(&[OscType::String(
        r#"{"name":"bad","members":[{"def":"missing"}]}"#.into(),
    )])
    .unwrap();
    for i in 0..(part.auto_capacity * 3) {
        let mut cmds = Vec::new();
        let err = t
            .translate(
                &msg(
                    "/graph_new",
                    vec![
                        OscType::String("bad".into()),
                        OscType::Int(-1),
                        OscType::Int(0),
                        OscType::Int(0),
                    ],
                ),
                &mut cmds,
            )
            .unwrap_err();
        assert!(
            !err.contains("out of auto node ids"),
            "iteration {i}: ids leaked into: {err}"
        );
        assert!(cmds.is_empty(), "a failed instantiation emits nothing");
    }
    // The range is still fully available for a real spawn.
    s_new_auto(&mut t).expect("no id was lost to the failures");
}
