use clausters_core::ids::{IdShare, ServerShape};
use clausters_core::osc::decode_packet;
use clausters_document::events::Event;
use serde_json::json;

use super::*;

const SR: f64 = 100.0;

fn ids() -> IdSpaces {
    IdSpaces::new(
        ServerShape {
            max_nodes: 1024,
            audio_buses: 1024,
            outputs: 2,
            control_buses: 16384,
            buffers: 1024,
        },
        IdShare::WHOLE,
    )
}

fn sequence() -> EventSequence {
    let mut seq = EventSequence::new(vec![
        Event::new(
            0.0,
            json!({"midinote": 60, "sustain": 1.0, "instrument": "default"}),
        ),
        Event::new(
            2.0,
            json!({"midinote": 64, "sustain": 1.0, "instrument": "default"}),
        ),
        Event::new(1.0, json!({"type": "osc", "addr": "/cue", "args": [1]})),
    ]);
    seq.tempo_map = Some(TempoMap::new(2.0));
    seq
}

fn at(clock: i64) -> At {
    At {
        clock,
        rate: SR,
        latency: 0.1,
    }
}

/// Every `/sched_atTransport` the steps send, as the clock sample and the
/// address of the message it carries.
fn stamped(steps: &[Step]) -> Vec<(i64, String)> {
    steps
        .iter()
        .filter_map(|step| match step {
            Step::Send(m) if m.addr == "/sched_atTransport" => {
                let (OscType::Long(sample), OscType::Blob(bytes)) = (&m.args[1], &m.args[2]) else {
                    return None;
                };
                let OscPacket::Bundle(bundle) = decode_packet(bytes).ok()? else {
                    return None;
                };
                let OscPacket::Message(inner) = &bundle.content[0] else {
                    return None;
                };
                Some((*sample, inner.addr.clone()))
            }
            _ => None,
        })
        .collect()
}

fn addrs(steps: &[Step]) -> Vec<String> {
    steps
        .iter()
        .filter_map(|s| match s {
            Step::Send(m) => Some(m.addr.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_play_plans_every_event_on_the_transports_clock() {
    let mut p = NotesPlayback::new(NOTES_EDITOR_TRANSPORT);
    let steps = p.play(&sequence(), 0.0, at(1000), &mut ids()).unwrap();
    let sent = addrs(&steps);
    assert!(sent.contains(&"/transport_group".to_string()), "{sent:?}");
    assert!(sent.contains(&"/sched_clear".to_string()));
    assert_eq!(sent.last().unwrap(), "/transport_play");
    // Two beats a second, a hundred samples a second, ten of latency: the
    // first note at 1010 with its release half a second on, the marker at 1060,
    // the second note at 1110.
    assert_eq!(
        stamped(&steps),
        vec![
            (1010, "/synth_new".into()),
            (1060, "/node_set".into()),
            (1060, "/cue".into()),
            (1110, "/synth_new".into()),
            (1160, "/node_set".into()),
        ]
    );
    assert!(p.rolling());
}

#[test]
fn a_note_starts_in_the_governed_group() {
    let mut p = NotesPlayback::new(NOTES_EDITOR_TRANSPORT);
    let steps = p.play(&sequence(), 0.0, at(0), &mut ids()).unwrap();
    let group = p.applier.node(GOVERNED).unwrap();
    let Step::Send(m) = steps
        .iter()
        .find(|s| matches!(s, Step::Send(m) if m.addr == "/sched_atTransport"))
        .unwrap()
    else {
        unreachable!()
    };
    let OscType::Blob(bytes) = &m.args[2] else {
        panic!()
    };
    let OscPacket::Bundle(b) = decode_packet(bytes).unwrap() else {
        panic!()
    };
    let OscPacket::Message(s_new) = &b.content[0] else {
        panic!()
    };
    assert_eq!(s_new.args[2], OscType::Int(1), "the tail");
    assert_eq!(s_new.args[3], OscType::Int(group));
}

/// An edit while it plays: the plan is written again from where the
/// transport stands, and the note already sounding keeps its release.
#[test]
fn a_replan_keeps_the_release_of_what_is_sounding() {
    let mut p = NotesPlayback::new(NOTES_EDITOR_TRANSPORT);
    let mut ids = ids();
    p.play(&sequence(), 0.0, at(1000), &mut ids).unwrap();
    let mut edited = sequence();
    edited
        .edit(clausters_document::EventsIntent::Move {
            id: 2,
            at: clausters_document::Beat(3.0),
        })
        .unwrap();
    // A quarter second in: position sample 25, clock 1025. The first note has
    // started and its release is at 1060.
    let steps = p.replan(&edited, 25, at(1025), &mut ids).unwrap();
    assert_eq!(addrs(&steps)[0], "/sched_clear");
    assert_eq!(
        stamped(&steps),
        vec![
            (1060, "/node_set".into()),
            (1060, "/cue".into()),
            (1160, "/synth_new".into()),
            (1210, "/node_set".into()),
        ],
        "the release, then the plan from 0.25 s on: the marker, and the note moved to beat 3"
    );
}

#[test]
fn a_stop_frees_the_notes_and_goes_back() {
    let mut p = NotesPlayback::new(NOTES_EDITOR_TRANSPORT);
    let mut ids = ids();
    p.play(&sequence(), 0.0, at(0), &mut ids).unwrap();
    let steps = p.stop(&sequence(), 1.0, SR);
    let sent = addrs(&steps);
    assert_eq!(sent[0], "/transport_stop");
    assert!(sent.contains(&"/group_freeAll".to_string()));
    assert_eq!(sent.last().unwrap(), "/transport_locateSample");
    assert!(!p.rolling());
}

#[test]
fn the_door_answers_steps() {
    let mut p = NotesPlayback::new(NOTES_EDITOR_TRANSPORT);
    let answer: Value = serde_json::from_str(&call_json(
        &mut p,
        &sequence(),
        r#"{"verb": "play", "from": 0, "clock": 0, "rate": 100}"#,
        &mut ids(),
    ))
    .unwrap();
    // The bundles come back from the JSON byte for byte, whatever tag they
    // travelled under.
    let back = crate::apply::steps_from_json(&answer["steps"]);
    let again = NotesPlayback::new(NOTES_EDITOR_TRANSPORT)
        .play(&sequence(), 0.0, at(0), &mut ids())
        .unwrap();
    assert_eq!(stamped(&back), stamped(&again));
    // Four bytes that read as a NaN travel as hex.
    let nan = crate::apply::arg_json(&OscType::Blob(vec![0x00, 0x00, 0xc0, 0x7f]));
    assert_eq!(nan["x"], "0000c07f");
    assert_eq!(
        crate::apply::arg_from_json(&nan),
        Some(OscType::Blob(vec![0x00, 0x00, 0xc0, 0x7f]))
    );
    let bad: Value = serde_json::from_str(&call_json(
        &mut p,
        &sequence(),
        r#"{"verb": "nope"}"#,
        &mut ids(),
    ))
    .unwrap();
    assert!(bad["error"].is_string());
}
