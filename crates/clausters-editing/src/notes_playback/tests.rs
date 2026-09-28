use clausters_core::ids::{IdShare, ServerShape};
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

/// Two notes and a marker, two beats a second.
fn sequence() -> EventSequence {
    let mut seq = EventSequence::new(vec![
        Event::new(
            0.0,
            json!({"midinote": 69, "sustain": 1.0, "instrument": "default", "amp": 0.2}),
        ),
        Event::new(
            2.0,
            json!({"midinote": 64, "sustain": 1.0, "instrument": "default"}),
        ),
        Event::new(
            1.0,
            json!({"type": "osc", "addr": "/cue", "args": [1, "a"]}),
        ),
    ]);
    seq.tempo_map = Some(TempoMap::new(2.0));
    seq
}

fn addrs(steps: &[Step]) -> Vec<String> {
    steps
        .iter()
        .filter_map(|step| match step {
            Step::Send(m) => Some(m.addr.clone()),
            _ => None,
        })
        .collect()
}

/// The data of the `/lane_set` among `steps`.
fn lane_data(steps: &[Step]) -> Option<Value> {
    steps.iter().find_map(|step| match step {
        Step::Send(m) if m.addr == "/lane_set" => match &m.args[1] {
            OscType::String(json) => serde_json::from_str(json).ok(),
            _ => None,
        },
        _ => None,
    })
}

/// **A sequence is placed through its own tempo map, and its lane data is
/// the core's rendering of each note** -- in samples, the def, the pitch as
/// `freq`, the level as `amp`, and how it is released.
#[test]
fn a_sequence_becomes_lane_data_in_samples() {
    let data = data(&placed(&sequence()), SR);
    // Two beats a second at 100 samples a second: beat 2 is sample 100.
    let notes = data["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0][0], json!(0));
    assert_eq!(notes[0][1], json!(50));
    assert_eq!(notes[0][2], json!("default"));
    assert!((notes[0][3]["freq"].as_f64().unwrap() - 440.0).abs() < 1e-3);
    assert!((notes[0][3]["amp"].as_f64().unwrap() - 0.2).abs() < 1e-6);
    assert_eq!(notes[0][4], json!("gate"), "the default def is gated");
    assert_eq!(
        (notes[1][0].clone(), notes[1][1].clone()),
        (json!(100), json!(150))
    );
    assert_eq!(data["messages"], json!([[50, "/cue", 1, "a"]]));
}

/// **A play makes the lane once, in the governed group, gives it the
/// sequence, and hands the rest to the transport**: its end mark at the
/// sequence's end, a locate and a roll -- no clock and no stamped bundle.
#[test]
fn a_play_is_the_lanes_data_and_the_transports_verbs() {
    let mut playback = NotesPlayback::new(NOTES_EDITOR_TRANSPORT);
    let mut ids = ids();
    let steps = playback.play(&sequence(), 1.0, SR, &mut ids).unwrap();
    let sent = addrs(&steps);
    let lane_new = steps
        .iter()
        .find_map(|s| match s {
            Step::Send(m) if m.addr == "/lane_new" => Some(m.args.clone()),
            _ => None,
        })
        .expect("a lane");
    let group = playback.applier.node(GOVERNED).unwrap();
    assert_eq!(
        lane_new,
        vec![
            OscType::Int(NOTES_EDITOR_TRANSPORT),
            OscType::Int(group),
            OscType::Int(group)
        ],
        "on its transport, named by and playing into the governed group"
    );
    assert!(lane_data(&steps).is_some());
    for addr in [
        "/transport_end",
        "/transport_locateSample",
        "/transport_play",
    ] {
        assert!(sent.contains(&addr.to_string()), "{addr} in {sent:?}");
    }
    assert!(!sent.contains(&"/sched_atTransport".to_string()));
    assert!(playback.rolling());

    // A second play makes nothing again.
    let again = addrs(&playback.play(&sequence(), 0.0, SR, &mut ids).unwrap());
    assert!(!again.contains(&"/lane_new".to_string()));
}

/// **An edit is the lane's new data**, and nothing before a play.
#[test]
fn an_update_sends_the_lane_its_data() {
    let mut playback = NotesPlayback::new(NOTES_EDITOR_TRANSPORT);
    assert!(playback.update(&sequence(), SR).is_empty(), "no lane yet");
    playback.play(&sequence(), 0.0, SR, &mut ids()).unwrap();
    let mut edited = sequence();
    edited.events.pop();
    let steps = playback.update(&edited, SR);
    assert_eq!(
        lane_data(&steps).unwrap()["notes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(addrs(&steps).contains(&"/transport_end".to_string()));
}

/// **A stop goes back, and a close frees the lane and the groups.**
#[test]
fn a_stop_goes_back_and_a_close_frees_the_lane() {
    let mut playback = NotesPlayback::new(NOTES_EDITOR_TRANSPORT);
    let mut ids = ids();
    playback.play(&sequence(), 0.0, SR, &mut ids).unwrap();
    assert_eq!(
        addrs(&playback.stop(&sequence(), 2.0, SR)),
        ["/transport_stop", "/transport_locateSample"]
    );
    let closed = addrs(&playback.close(&mut ids).unwrap());
    assert!(closed.contains(&"/lane_free".to_string()));
    assert!(closed.contains(&"/node_free".to_string()));
    assert!(playback.close(&mut ids).unwrap().is_empty(), "once");
}

/// **A cue locates a stopped transport on the beat's sample** and leaves a
/// rolling one alone.
#[test]
fn a_cue_locates_a_stopped_transport_and_leaves_a_rolling_one() {
    let mut playback = NotesPlayback::new(NOTES_EDITOR_TRANSPORT);
    let cued = playback.cue(&sequence(), 3.0, SR);
    assert_eq!(addrs(&cued), ["/transport_locateSample"]);
    // Two beats a second at 100 samples a second: beat 3 is sample 150.
    let Step::Send(locate) = &cued[0] else {
        panic!("a send");
    };
    assert!(matches!(locate.args.last(), Some(OscType::Long(150))));
    playback.play(&sequence(), 0.0, SR, &mut ids()).unwrap();
    assert!(playback.cue(&sequence(), 1.0, SR).is_empty());
}

/// The door answers steps, and names the verbs it has.
#[test]
fn the_door_answers_steps() {
    let mut playback = NotesPlayback::new(NOTES_EDITOR_TRANSPORT);
    let mut ids = ids();
    let answer: Value = serde_json::from_str(&call_json(
        &mut playback,
        &sequence(),
        r#"{"verb": "play", "from": 0, "rate": 100}"#,
        &mut ids,
    ))
    .unwrap();
    assert!(answer["steps"].as_array().is_some_and(|s| !s.is_empty()));
    let refused: Value = serde_json::from_str(&call_json(
        &mut playback,
        &sequence(),
        r#"{"verb": "replan"}"#,
        &mut ids,
    ))
    .unwrap();
    assert!(
        refused["error"].is_string(),
        "replanning is the transport's now"
    );
}
