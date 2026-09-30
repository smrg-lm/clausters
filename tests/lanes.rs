//! **Event lanes**: notes and messages the transport plays by its position,
//! the way a reader plays a take.
//!
//! Pulled mode (`ClaustersHeadless`), so the serving turn that feeds a lane and
//! the blocks that fire it run on one thread in a fixed order, and every
//! assertion is about an exact sample. A note is a synth writing a constant to
//! bus 0 while it lives, so the output says when it started and when it was
//! released.

#![cfg(all(feature = "synth", feature = "embed"))]

use clausters::embed::ClaustersHeadless;
use clausters::rosc::{OscMessage, OscPacket, OscType, encoder};
use clausters::server::engine::BLOCK_SIZE;

const SR: f64 = 48_000.0;
const CHANNELS: usize = 2;
const GROUP: i32 = 100;
const LANE: i32 = 1;

fn msg(addr: &str, args: Vec<OscType>) -> Vec<u8> {
    encoder::encode(&OscPacket::Message(OscMessage {
        addr: addr.into(),
        args,
    }))
    .unwrap()
}

fn send(server: &mut ClaustersHeadless, addr: &str, args: Vec<OscType>) {
    assert!(server.send(&msg(addr, args)), "{addr} was not taken");
}

/// Channel 0 of `blocks` pulled blocks.
fn pull(server: &mut ClaustersHeadless, blocks: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; blocks * BLOCK_SIZE * CHANNELS];
    server.process_block(&mut out).unwrap();
    out.as_chunks::<CHANNELS>().0.iter().map(|f| f[0]).collect()
}

/// The spans of frames that sound, as `(first, one past the last)`.
fn sounding(frames: &[f32]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut open = None;
    for (i, x) in frames.iter().enumerate() {
        match (open, *x != 0.0) {
            (None, true) => open = Some(i),
            (Some(from), false) => {
                spans.push((from, i));
                open = None;
            }
            _ => {}
        }
    }
    if let Some(from) = open {
        spans.push((from, frames.len()));
    }
    spans
}

/// A server with a `dc` def, a group transport 0 governs, and lane 1 on it.
fn server() -> ClaustersHeadless {
    let mut server = ClaustersHeadless::new(SR, CHANNELS, 0.0).unwrap();
    let dc = r#"{
        "name": "dc",
        "controls": [{"name": "level", "default": 0.5}],
        "ugens": [
            {"kind": "Out", "inputs": [{"const": 0.0}, {"control": 0}]}
        ]
    }"#;
    send(
        &mut server,
        "/def_send",
        vec![
            OscType::String("synth".into()),
            OscType::Blob(dc.as_bytes().to_vec()),
        ],
    );
    send(
        &mut server,
        "/group_new",
        vec![OscType::Int(GROUP), OscType::Int(1), OscType::Int(0)],
    );
    send(
        &mut server,
        "/transport_group",
        vec![OscType::Int(0), OscType::Int(GROUP)],
    );
    send(
        &mut server,
        "/lane_new",
        vec![OscType::Int(0), OscType::Int(LANE), OscType::Int(GROUP)],
    );
    server
}

fn set_lane(server: &mut ClaustersHeadless, notes: &[(u64, u64)]) {
    let notes: Vec<String> = notes
        .iter()
        .map(|(a, b)| format!(r#"[{a}, {b}, "dc", {{"level": 0.5}}, "free"]"#))
        .collect();
    let json = format!(r#"{{"notes": [{}]}}"#, notes.join(", "));
    send(
        server,
        "/lane_set",
        vec![OscType::Int(LANE), OscType::String(json)],
    );
}

fn play(server: &mut ClaustersHeadless) {
    send(server, "/transport_play", vec![OscType::Int(0)]);
}

/// **A note sounds from the sample the position reaches to the sample its
/// length ends on**, mid-block at both ends.
#[test]
fn a_lanes_note_sounds_on_its_exact_samples() {
    let mut server = server();
    set_lane(&mut server, &[(4808, 9608)]);
    play(&mut server);
    let out = pull(&mut server, 200);
    assert_eq!(sounding(&out), vec![(4808, 9608)]);
}

/// **A loop plays the lane again on every pass** with no client in the loop:
/// the feed follows the wrap, and each pass fires on its own sample.
#[test]
fn a_loop_plays_the_lane_on_every_pass() {
    let mut server = server();
    set_lane(&mut server, &[(1000, 2000)]);
    send(
        &mut server,
        "/transport_loop",
        vec![OscType::Int(0), OscType::Long(0), OscType::Long(9600)],
    );
    play(&mut server);
    // Four passes of 9600 samples.
    let out = pull(&mut server, 4 * 9600 / BLOCK_SIZE);
    assert_eq!(
        sounding(&out),
        vec![(1000, 2000), (10600, 11600), (20200, 21200), (29800, 30800)]
    );
}

/// **A locate releases what the lane was sounding**, and the notes ahead of
/// the new position fire where it reaches them.
#[test]
fn a_locate_releases_the_sounding_notes_and_plays_on_from_there() {
    let mut server = server();
    set_lane(&mut server, &[(0, 48_000), (10_000, 12_000)]);
    play(&mut server);
    // 100 blocks in, the first note is sounding.
    let before = pull(&mut server, 100);
    assert_eq!(sounding(&before), vec![(0, 6400)]);
    send(
        &mut server,
        "/transport_locateSample",
        vec![OscType::Int(0), OscType::Long(9000)],
    );
    // The locate lands on the next block: the note is released there, and the
    // second one fires 1000 samples later.
    let after = pull(&mut server, 100);
    assert_eq!(sounding(&after), vec![(1000, 3000)]);
}

/// **An edit is heard at once, and what sounds keeps its release**: moving a
/// note ahead of the position plays it where it now is, while the note that
/// is sounding ends where it would have.
#[test]
fn an_edit_mid_pass_is_heard_and_a_sounding_note_keeps_its_release() {
    let mut server = server();
    set_lane(&mut server, &[(0, 9600), (20_000, 21_000)]);
    play(&mut server);
    pull(&mut server, 100);
    set_lane(&mut server, &[(0, 9600), (12_800, 13_800)]);
    let out = pull(&mut server, 300);
    // The sounding note ends on 9600 (3200 into this pull), and the moved one
    // plays at 12800, not at 20000.
    assert_eq!(sounding(&out), vec![(0, 3200), (6400, 7400)]);
}

/// **A client's own `/sched_clear` leaves a lane alone**: the release of the
/// note the lane is sounding stays, and the lane goes on playing.
#[test]
fn a_clients_sched_clear_leaves_the_lane_alone() {
    let mut server = server();
    set_lane(&mut server, &[(0, 9600), (12_800, 13_800)]);
    play(&mut server);
    pull(&mut server, 50);
    send(
        &mut server,
        "/sched_clear",
        vec![OscType::String("transport".into()), OscType::Int(0)],
    );
    let out = pull(&mut server, 250);
    assert_eq!(sounding(&out), vec![(0, 6400), (9600, 10600)]);
}

/// **A stopped transport holds the lane with it**, and a play goes on from
/// the position it held.
#[test]
fn a_stop_holds_the_lane_and_a_play_goes_on() {
    let mut server = server();
    set_lane(&mut server, &[(6400, 7400)]);
    play(&mut server);
    pull(&mut server, 50);
    send(&mut server, "/transport_stop", vec![OscType::Int(0)]);
    let stopped = pull(&mut server, 100);
    assert!(sounding(&stopped).is_empty(), "nothing while it is stopped");
    play(&mut server);
    // It stopped at 3200, so the note is 3200 samples into this pull.
    let out = pull(&mut server, 100);
    assert_eq!(sounding(&out), vec![(3200, 4200)]);
}

/// **A stop releases what the lane is sounding**, as a DAW's stop sends its
/// note-offs, and the next play does not bring it back: a voice frozen with
/// its note held would sound again, mid-release, wherever the play starts.
/// The notes are made in a group the transport does not govern, so nothing
/// of theirs freezes.
#[test]
fn a_stop_releases_the_sounding_notes_and_a_play_does_not_bring_them_back() {
    let mut server = server();
    send(
        &mut server,
        "/group_new",
        vec![OscType::Int(200), OscType::Int(0), OscType::Int(0)],
    );
    send(
        &mut server,
        "/lane_new",
        vec![OscType::Int(0), OscType::Int(2), OscType::Int(200)],
    );
    let json = r#"{"notes": [[0, 48000, "dc", {"level": 0.5}, "free"]]}"#;
    send(
        &mut server,
        "/lane_set",
        vec![OscType::Int(2), OscType::String(json.into())],
    );
    play(&mut server);
    assert_eq!(sounding(&pull(&mut server, 50)), vec![(0, 3200)]);
    send(&mut server, "/transport_stop", vec![OscType::Int(0)]);
    assert!(
        sounding(&pull(&mut server, 50)).is_empty(),
        "released on the stop"
    );
    play(&mut server);
    assert!(
        sounding(&pull(&mut server, 50)).is_empty(),
        "and not sounding again when the transport rolls on"
    );
}

/// **The end mark is a stop**, and releases what the lane sounds on it.
#[test]
fn the_end_mark_releases_what_the_lane_sounds() {
    let mut server = server();
    send(
        &mut server,
        "/group_new",
        vec![OscType::Int(200), OscType::Int(0), OscType::Int(0)],
    );
    send(
        &mut server,
        "/lane_new",
        vec![OscType::Int(0), OscType::Int(2), OscType::Int(200)],
    );
    let json = r#"{"notes": [[0, 48000, "dc", {"level": 0.5}, "free"]]}"#;
    send(
        &mut server,
        "/lane_set",
        vec![OscType::Int(2), OscType::String(json.into())],
    );
    send(
        &mut server,
        "/transport_end",
        vec![OscType::Int(0), OscType::Long(6400), OscType::Long(0)],
    );
    play(&mut server);
    assert_eq!(sounding(&pull(&mut server, 200)), vec![(0, 6400)]);
}

/// Binds MIDI channel 0 to `dc`, its voices at the tail of the governed group,
/// and a controller 7 to its `level`.
fn bind_midi(server: &mut ClaustersHeadless) {
    send(
        server,
        "/midi_bind",
        vec![
            OscType::Int(0),
            OscType::String("dc".into()),
            OscType::Int(GROUP),
            OscType::Int(1),
        ],
    );
    send(
        server,
        "/midi_map",
        vec![
            OscType::Int(0),
            OscType::String("cc7".into()),
            OscType::String("level".into()),
        ],
    );
}

fn set_midi(server: &mut ClaustersHeadless, midi: &[(u64, [u8; 3])]) {
    let midi: Vec<String> = midi
        .iter()
        .map(|(at, [a, b, c])| format!("[{at}, {a}, {b}, {c}]"))
        .collect();
    let json = format!(r#"{{"midi": [{}]}}"#, midi.join(", "));
    send(
        server,
        "/lane_set",
        vec![OscType::Int(LANE), OscType::String(json)],
    );
}

/// **A lane's MIDI note-on and its note-off sound the channel's instrument**
/// from one sample to the other, as the live input would have.
#[test]
fn a_lanes_midi_note_sounds_its_channels_instrument_on_its_samples() {
    let mut server = server();
    bind_midi(&mut server);
    set_midi(
        &mut server,
        &[(4808, [0x90, 60, 100]), (9608, [0x80, 60, 0])],
    );
    play(&mut server);
    let out = pull(&mut server, 200);
    assert_eq!(sounding(&out), vec![(4808, 9608)]);
}

/// **A locate releases a lane's MIDI note** as it releases a lane's synth.
#[test]
fn a_locate_releases_a_lanes_midi_note() {
    let mut server = server();
    bind_midi(&mut server);
    set_midi(
        &mut server,
        &[(0, [0x90, 60, 100]), (48_000, [0x80, 60, 0])],
    );
    play(&mut server);
    let before = pull(&mut server, 100);
    assert_eq!(sounding(&before), vec![(0, 6400)]);
    send(
        &mut server,
        "/transport_locateSample",
        vec![OscType::Int(0), OscType::Long(9000)],
    );
    assert_eq!(sounding(&pull(&mut server, 100)), Vec::new());
}

/// **A controller on the lane moves the voice it sounds over**, on its
/// sample, through the control the binding maps it to.
#[test]
fn a_lanes_controller_moves_its_sounding_voice() {
    let mut server = server();
    bind_midi(&mut server);
    set_midi(
        &mut server,
        &[
            (1000, [0x90, 60, 100]),
            (6000, [0xB0, 7, 127]),
            (9000, [0x80, 60, 0]),
        ],
    );
    play(&mut server);
    let out = pull(&mut server, 200);
    assert_eq!(out[5999], 0.5);
    assert_eq!(out[6000], 1.0);
    assert_eq!(sounding(&out), vec![(1000, 9000)]);
}

/// **A note on an unbound channel sounds nothing**, and a note-on the lane
/// never turns off is not played.
#[test]
fn an_unbound_channel_and_an_unended_note_sound_nothing() {
    let mut server = server();
    bind_midi(&mut server);
    set_midi(
        &mut server,
        &[
            (1000, [0x91, 60, 100]),
            (2000, [0x81, 60, 0]),
            (3000, [0x90, 62, 100]),
        ],
    );
    play(&mut server);
    assert_eq!(sounding(&pull(&mut server, 100)), Vec::new());
}

/// **A lane's MPE plays per note**: in a zone, a member's bend retunes its own
/// note on its sample, as the live input would. The def writes its `freq` to
/// the output, so the output is the pitch.
#[test]
fn a_lanes_mpe_bend_retunes_its_note_on_its_sample() {
    let mut server = server();
    let pitch = r#"{
        "name": "pitch",
        "controls": [{"name": "freq", "default": 0.0}],
        "ugens": [
            {"kind": "Out", "inputs": [{"const": 0.0}, {"control": 0}]}
        ]
    }"#;
    send(
        &mut server,
        "/def_send",
        vec![
            OscType::String("synth".into()),
            OscType::Blob(pitch.as_bytes().to_vec()),
        ],
    );
    send(
        &mut server,
        "/midi_bindZone",
        vec![
            OscType::Int(0),
            OscType::Int(15),
            OscType::String("pitch".into()),
            OscType::Int(GROUP),
            OscType::Int(1),
        ],
    );
    let up = clausters_midi::mpe::bend_message(1, 12.0, 48.0);
    set_midi(
        &mut server,
        &[(1000, [0x91, 69, 100]), (3000, up), (6000, [0x81, 69, 0])],
    );
    play(&mut server);
    let out = pull(&mut server, 200);
    assert!((out[2999] - 440.0).abs() < 0.01, "{}", out[2999]);
    assert!((out[3000] - 880.0).abs() < 1.0, "{}", out[3000]);
    assert_eq!(sounding(&out), vec![(1000, 6000)]);
}

// ---- a note's curves, through its channel's graph ----

/// **A note in its channel's graph hears both scopes**: the channel's bend
/// and its own add (an octave each: 220 Hz sounds 880), and its own curve
/// drives the control it names (`level`, halving it). The probe writes
/// `freq * level` while it lives, so the output is the combination itself.
#[test]
fn a_note_in_a_graph_hears_its_channels_curves_and_its_own() {
    use clausters_core::event_graph::{
        Shape, channel_graph, curve_def, hold_def, local_def, note_graph, pitch_def,
    };

    let mut server = server();
    let probe = r#"{
        "name": "probe",
        "controls": [{"name": "freq", "default": 440.0},
                     {"name": "level", "default": 1.0}],
        "ugens": [
            {"kind": "BinaryOpUGen", "op": "mul", "inputs": [{"control": 0}, {"control": 1}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    }"#;
    for def in [
        probe.to_string(),
        curve_def().to_string(),
        local_def().to_string(),
        hold_def().to_string(),
        pitch_def().to_string(),
    ] {
        send(
            &mut server,
            "/def_send",
            vec![
                OscType::String("synth".into()),
                OscType::Blob(def.into_bytes()),
            ],
        );
    }
    let shape = Shape {
        def: "probe".into(),
        controls: vec!["freq".into()],
        own: vec!["bend".into(), "level".into()],
        lanes: vec!["bend".into()],
    };
    let note = note_graph(&shape);
    let channel = channel_graph(&["bend".to_string()], std::slice::from_ref(&note));
    for graph in [&note, &channel] {
        send(
            &mut server,
            "/def_send",
            vec![
                OscType::String("graph".into()),
                OscType::Blob(graph.to_string().into_bytes()),
            ],
        );
    }
    // One-sample tables hold their value: the channel's bend, the note's bend
    // and the note's level.
    for (buffer, value) in [(0, 12.0), (1, 12.0), (2, 0.5)] {
        send(
            &mut server,
            "/buffer_alloc",
            vec![OscType::Int(buffer), OscType::Int(1), OscType::Int(1)],
        );
        pull(&mut server, 1);
        send(
            &mut server,
            "/buffer_set",
            vec![OscType::Int(buffer), OscType::Int(0), OscType::Float(value)],
        );
    }
    let name = channel["name"].as_str().unwrap().to_string();
    send(
        &mut server,
        "/graph_new",
        vec![
            OscType::String(name),
            OscType::Int(500),
            OscType::Int(1),
            OscType::Int(0),
            OscType::String("lane/bend/buf".into()),
            OscType::Float(0.0),
        ],
    );
    let json = r#"{"notes": [[4808, 9608, {"graph": 500, "slot": "note.0"},
        {"freq": 220, "bend/buf": 1, "level/buf": 2}, "free"]]}"#;
    send(
        &mut server,
        "/lane_set",
        vec![OscType::Int(LANE), OscType::String(json.into())],
    );
    play(&mut server);
    let out = pull(&mut server, 200);
    assert_eq!(sounding(&out), vec![(4808, 9608)]);
    // The first block reads what the readers wrote in it; every sample after
    // the note's first is the combination.
    let held = &out[4808 + 64..9608];
    assert!(
        held.iter().all(|x| (*x - 440.0).abs() < 1e-2),
        "880 Hz at half level, got {:?}",
        &held[..4]
    );
}

/// **A note in a graph is released through its slot's `gate`**, and its
/// envelope closing ends it: a gated voice started with no `gate` of its own
/// still falls silent a release after its end.
#[test]
fn a_note_in_a_graph_is_released_through_its_gate() {
    use clausters_core::event_graph::{
        Shape, channel_graph, curve_def, hold_def, local_def, note_graph, pitch_def,
    };

    let mut server = server();
    // `level * env`, a 10 ms release that frees the voice.
    let gated = r#"{
        "name": "gated",
        "controls": [{"name": "level", "default": 0.5}, {"name": "gate", "default": 1.0}],
        "ugens": [
            {"kind": "EnvGen", "inputs": [
                {"control": 1}, {"const": 1.0}, {"const": 0.0}, {"const": 1.0},
                {"const": 2.0}, {"const": 1.0}, {"const": 2.0}, {"const": 1.0},
                {"const": -1.0},
                {"const": 1.0}, {"const": 0.0}, {"const": 1.0}, {"const": 0.0},
                {"const": 0.0}, {"const": 0.01}, {"const": 1.0}, {"const": 0.0}
            ]},
            {"kind": "BinaryOpUGen", "op": "mul", "inputs": [{"control": 0}, {"ugen": 0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]
    }"#;
    for def in [
        gated.to_string(),
        curve_def().to_string(),
        local_def().to_string(),
        hold_def().to_string(),
        pitch_def().to_string(),
    ] {
        send(
            &mut server,
            "/def_send",
            vec![
                OscType::String("synth".into()),
                OscType::Blob(def.into_bytes()),
            ],
        );
    }
    let shape = Shape {
        def: "gated".into(),
        controls: vec!["level".into()],
        own: vec!["level".into()],
        lanes: Vec::new(),
    };
    let note = note_graph(&shape);
    let channel = channel_graph(&[], std::slice::from_ref(&note));
    for graph in [&note, &channel] {
        send(
            &mut server,
            "/def_send",
            vec![
                OscType::String("graph".into()),
                OscType::Blob(graph.to_string().into_bytes()),
            ],
        );
    }
    send(
        &mut server,
        "/buffer_alloc",
        vec![OscType::Int(0), OscType::Int(1), OscType::Int(1)],
    );
    pull(&mut server, 1);
    send(
        &mut server,
        "/buffer_set",
        vec![OscType::Int(0), OscType::Int(0), OscType::Float(0.25)],
    );
    let name = channel["name"].as_str().unwrap().to_string();
    send(
        &mut server,
        "/graph_new",
        vec![
            OscType::String(name),
            OscType::Int(500),
            OscType::Int(1),
            OscType::Int(0),
        ],
    );
    let json = r#"{"notes": [[4808, 9608, {"graph": 500, "slot": "note.0"},
        {"level": 0.5, "level/buf": 0}, "gate"]]}"#;
    send(
        &mut server,
        "/lane_set",
        vec![OscType::Int(LANE), OscType::String(json.into())],
    );
    play(&mut server);
    let out = pull(&mut server, 300);
    let spans = sounding(&out);
    assert_eq!(spans.len(), 1, "{spans:?}");
    let (from, to) = spans[0];
    assert_eq!(from, 4808);
    assert!(
        (9608..9608 + 480 + 64).contains(&to),
        "released at its end and silent a release later, not held: {to}"
    );
    assert!(
        out[4808 + 64..9608]
            .iter()
            .all(|x| (*x - 0.25).abs() < 1e-3),
        "{:?}",
        &out[4808 + 60..4808 + 70]
    );
}

/// **A stop holds the curves a releasing note reads**: the transport stops
/// and is located back to the start, where the channel's curve is lower, and
/// the note's release falls from where it was rather than stepping down to it.
#[test]
fn a_stop_holds_the_curves_a_releasing_note_reads() {
    use clausters_core::event_graph::{
        Shape, channel_graph, curve_def, hold_def, local_def, note_graph, pitch_def,
    };

    let mut server = server();
    // `level * env`, a 0.2 s linear release.
    let gated = r#"{
        "name": "gated",
        "controls": [{"name": "level", "default": 0.5}, {"name": "gate", "default": 1.0}],
        "ugens": [
            {"kind": "EnvGen", "inputs": [
                {"control": 1}, {"const": 1.0}, {"const": 0.0}, {"const": 1.0},
                {"const": 2.0}, {"const": 1.0}, {"const": 2.0}, {"const": 1.0},
                {"const": -1.0},
                {"const": 1.0}, {"const": 0.0}, {"const": 1.0}, {"const": 0.0},
                {"const": 0.0}, {"const": 0.2}, {"const": 1.0}, {"const": 0.0}
            ]},
            {"kind": "BinaryOpUGen", "op": "mul", "inputs": [{"control": 0}, {"ugen": 0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]
    }"#;
    for def in [
        gated.to_string(),
        curve_def().to_string(),
        local_def().to_string(),
        hold_def().to_string(),
        pitch_def().to_string(),
    ] {
        send(
            &mut server,
            "/def_send",
            vec![
                OscType::String("synth".into()),
                OscType::Blob(def.into_bytes()),
            ],
        );
    }
    let shape = Shape {
        def: "gated".into(),
        controls: Vec::new(),
        own: Vec::new(),
        lanes: vec!["level".into()],
    };
    let note = note_graph(&shape);
    let channel = channel_graph(&["level".to_string()], std::slice::from_ref(&note));
    for graph in [&note, &channel] {
        send(
            &mut server,
            "/def_send",
            vec![
                OscType::String("graph".into()),
                OscType::Blob(graph.to_string().into_bytes()),
            ],
        );
    }
    // The channel's level: 0.1 at the start, 0.8 from a tenth of a second on.
    send(
        &mut server,
        "/buffer_alloc",
        vec![OscType::Int(0), OscType::Int(2), OscType::Int(1)],
    );
    pull(&mut server, 1);
    send(
        &mut server,
        "/buffer_set",
        vec![
            OscType::Int(0),
            OscType::Int(0),
            OscType::Float(0.1),
            OscType::Int(1),
            OscType::Float(0.8),
        ],
    );
    // The channel is made where the notes editor makes it: in a group that
    // follows the transport, which a stop does not freeze.
    send(
        &mut server,
        "/group_new",
        vec![OscType::Int(200), OscType::Int(0), OscType::Int(0)],
    );
    send(
        &mut server,
        "/transport_follow",
        vec![OscType::Int(0), OscType::Int(200)],
    );
    let name = channel["name"].as_str().unwrap().to_string();
    send(
        &mut server,
        "/graph_new",
        vec![
            OscType::String(name),
            OscType::Int(500),
            OscType::Int(1),
            OscType::Int(200),
            OscType::String("lane/level/buf".into()),
            OscType::Float(0.0),
            OscType::String("lane/level/step".into()),
            OscType::Float(4800.0),
        ],
    );
    let json = r#"{"notes": [[9600, 96000, {"graph": 500, "slot": "note.0"}, {}, "gate"]]}"#;
    send(
        &mut server,
        "/lane_set",
        vec![OscType::Int(LANE), OscType::String(json.into())],
    );
    play(&mut server);
    let mut out = pull(&mut server, 400);
    assert!(
        (out[out.len() - 1] - 0.8).abs() < 1e-3,
        "the note at the channel's level: {:?}",
        &out[out.len() - 3..]
    );
    send(&mut server, "/transport_stop", vec![OscType::Int(0)]);
    send(
        &mut server,
        "/transport_locateSample",
        vec![OscType::Int(0), OscType::Long(0)],
    );
    out = pull(&mut server, 200);
    let steps = out
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(
        steps < 0.01,
        "the release falls from 0.8 without a step (largest {steps})"
    );
    assert!(out.iter().any(|x| *x > 0.5), "and it is the release of 0.8");
}

/// **A loop that ends inside a note releases it at the wrap**, and the next
/// pass plays it again from its start.
#[test]
fn a_loop_ending_inside_a_note_releases_it_at_the_wrap() {
    let mut server = server();
    set_lane(&mut server, &[(4000, 12_000)]);
    send(
        &mut server,
        "/transport_loop",
        vec![OscType::Int(0), OscType::Long(0), OscType::Long(9600)],
    );
    play(&mut server);
    let out = pull(&mut server, 3 * 9600 / BLOCK_SIZE);
    assert_eq!(
        sounding(&out),
        vec![(4000, 9600), (13_600, 19_200), (23_200, 28_800)]
    );
}

/// **A loop that ends where the last note does plays it on every pass.**
#[test]
fn a_loop_ending_at_the_last_notes_end_plays_it_every_pass() {
    let mut server = server();
    set_lane(&mut server, &[(1000, 9600)]);
    send(
        &mut server,
        "/transport_loop",
        vec![OscType::Int(0), OscType::Long(0), OscType::Long(9600)],
    );
    play(&mut server);
    let out = pull(&mut server, 3 * 9600 / BLOCK_SIZE);
    assert_eq!(
        sounding(&out),
        vec![(1000, 9600), (10_600, 19_200), (20_200, 28_800)]
    );
}

/// **A loop's wrap leaves a releasing note its channel's last value**: the
/// channel's curve is low at the loop's start and high at its end, the note
/// held across the end is released on the seam, and its release falls from
/// the high value rather than stepping down to the low one.
#[test]
fn a_wrap_leaves_a_releasing_note_its_channels_last_value() {
    use clausters_core::event_graph::{
        Shape, channel_graph, curve_def, hold_def, local_def, note_graph, pitch_def,
    };

    let mut server = server();
    let gated = r#"{
        "name": "gated",
        "controls": [{"name": "level", "default": 0.5}, {"name": "gate", "default": 1.0}],
        "ugens": [
            {"kind": "EnvGen", "inputs": [
                {"control": 1}, {"const": 1.0}, {"const": 0.0}, {"const": 1.0},
                {"const": 2.0}, {"const": 1.0}, {"const": 2.0}, {"const": 1.0},
                {"const": -1.0},
                {"const": 1.0}, {"const": 0.0}, {"const": 1.0}, {"const": 0.0},
                {"const": 0.0}, {"const": 0.2}, {"const": 1.0}, {"const": 0.0}
            ]},
            {"kind": "BinaryOpUGen", "op": "mul", "inputs": [{"control": 0}, {"ugen": 0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]
    }"#;
    for def in [
        gated.to_string(),
        curve_def().to_string(),
        local_def().to_string(),
        hold_def().to_string(),
        pitch_def().to_string(),
    ] {
        send(
            &mut server,
            "/def_send",
            vec![
                OscType::String("synth".into()),
                OscType::Blob(def.into_bytes()),
            ],
        );
    }
    let shape = Shape {
        def: "gated".into(),
        controls: Vec::new(),
        own: Vec::new(),
        lanes: vec!["level".into()],
    };
    let note = note_graph(&shape);
    let channel = channel_graph(&["level".to_string()], std::slice::from_ref(&note));
    for graph in [&note, &channel] {
        send(
            &mut server,
            "/def_send",
            vec![
                OscType::String("graph".into()),
                OscType::Blob(graph.to_string().into_bytes()),
            ],
        );
    }
    // The channel's level: 0.1 until a tenth of a second, 0.8 after.
    send(
        &mut server,
        "/buffer_alloc",
        vec![OscType::Int(0), OscType::Int(2), OscType::Int(1)],
    );
    pull(&mut server, 1);
    send(
        &mut server,
        "/buffer_set",
        vec![
            OscType::Int(0),
            OscType::Int(0),
            OscType::Float(0.1),
            OscType::Int(1),
            OscType::Float(0.8),
        ],
    );
    send(
        &mut server,
        "/group_new",
        vec![OscType::Int(200), OscType::Int(0), OscType::Int(0)],
    );
    send(
        &mut server,
        "/transport_follow",
        vec![OscType::Int(0), OscType::Int(200)],
    );
    let name = channel["name"].as_str().unwrap().to_string();
    send(
        &mut server,
        "/graph_new",
        vec![
            OscType::String(name),
            OscType::Int(500),
            OscType::Int(1),
            OscType::Int(200),
            OscType::String("lane/level/buf".into()),
            OscType::Float(0.0),
            OscType::String("lane/level/step".into()),
            OscType::Float(4800.0),
        ],
    );
    // A note from 9600 past the loop's end at 19200.
    let json = r#"{"notes": [[9600, 30000, {"graph": 500, "slot": "note.0"}, {}, "gate"]]}"#;
    send(
        &mut server,
        "/lane_set",
        vec![OscType::Int(LANE), OscType::String(json.into())],
    );
    send(
        &mut server,
        "/transport_loop",
        vec![OscType::Int(0), OscType::Long(0), OscType::Long(19_200)],
    );
    play(&mut server);
    let out = pull(&mut server, 19_200 / BLOCK_SIZE + 60);
    assert!(
        (out[19_000] - 0.8).abs() < 1e-3,
        "at the channel's level before the seam"
    );
    let steps = out[19_000..]
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(
        steps < 0.01,
        "the release falls from 0.8 across the seam without a step (largest {steps})"
    );
}
