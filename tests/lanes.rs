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
