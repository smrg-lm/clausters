//! S12: the destructive edit verbs on the wire — `/buffer_gain` and
//! `/buffer_reverse`.
//!
//! The arithmetic itself is `clausters_core::edit`'s and is unit-tested there;
//! what these check is the wire: that a span in **frames** reaches the right
//! samples, that a batch of edits on one buffer builds on each other rather
//! than each on the pre-batch contents, and that a span past the end fails
//! instead of quietly doing less.
//!
//! Driven through an offline session, which is the cheapest driver that owns a
//! whole server; the commands themselves are ordinary buffer commands and run
//! on any server.

#![cfg(feature = "synth")]

use clausters::rosc::{OscMessage, OscType};
use clausters::server::nrtsession::{NrtSession, SessionConfig};

const CHANNELS: usize = 2;
const FRAMES: usize = 8;

fn session() -> NrtSession {
    NrtSession::open(&SessionConfig {
        sample_rate: 48_000.0,
        channels: CHANNELS,
        ..Default::default()
    })
    .expect("open")
}

fn samples_blob(values: &[f32]) -> OscType {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for v in values {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    OscType::Blob(bytes)
}

/// A buffer holding 1..=frames*channels, so every sample says where it is.
fn loaded(s: &mut NrtSession) -> Vec<f32> {
    let ramp: Vec<f32> = (0..FRAMES * CHANNELS).map(|i| i as f32 + 1.0).collect();
    send(
        s,
        "/buffer_alloc",
        vec![
            OscType::Int(0),
            OscType::Int(FRAMES as i32),
            OscType::Int(CHANNELS as i32),
        ],
    );
    s.settle_for(4);
    send(
        s,
        "/buffer_setRange",
        vec![OscType::Int(0), OscType::Int(0), samples_blob(&ramp)],
    );
    s.settle_for(4);
    ramp
}

fn send(s: &mut NrtSession, addr: &str, args: Vec<OscType>) {
    assert!(s.send_msg(addr, args).expect("encode"), "ring full");
}

/// Reads the whole buffer back over the wire.
fn read_back(s: &mut NrtSession) -> Vec<f32> {
    send(
        s,
        "/buffer_getRange",
        vec![
            OscType::Int(0),
            OscType::Int(0),
            OscType::Int((FRAMES * CHANNELS) as i32),
        ],
    );
    let m = reply(s, "/buffer_getRange.reply").expect("the buffer reads back");
    let OscType::Blob(bytes) = m.args.last().expect("a blob") else {
        panic!("expected a blob, got {:?}", m.args)
    };
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

fn reply(s: &mut NrtSession, addr: &str) -> Option<OscMessage> {
    let mut buf = vec![0u8; 1 << 16];
    for _ in 0..64 {
        while let Some(len) = s.poll_into(&mut buf) {
            if let Ok(clausters::rosc::OscPacket::Message(m)) =
                clausters::osc::decode_packet(&buf[..len])
                && m.addr == addr
            {
                return Some(m);
            }
        }
        s.settle();
    }
    None
}

/// Collects every `/fail` that has arrived, with its reason.
fn fails(s: &mut NrtSession) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = vec![0u8; 1 << 16];
    s.settle_for(4);
    while let Some(len) = s.poll_into(&mut buf) {
        if let Ok(clausters::rosc::OscPacket::Message(m)) =
            clausters::osc::decode_packet(&buf[..len])
            && m.addr == "/fail"
        {
            out.push(format!("{:?}", m.args));
        }
    }
    out
}

#[test]
fn gain_scales_a_span_in_frames_not_in_flat_samples() {
    let mut s = session();
    let ramp = loaded(&mut s);
    // Frames 1..3 — samples 2..6 flat, which is the whole point of the unit.
    send(
        &mut s,
        "/buffer_gain",
        vec![
            OscType::Int(0),
            OscType::Int(1),
            OscType::Int(2),
            OscType::Float(0.5),
        ],
    );
    s.settle_for(4);

    let mut expected = ramp.clone();
    clausters_core::edit::gain(
        &mut expected,
        CHANNELS,
        1,
        2,
        clausters_core::edit::Fade::constant(0.5),
    )
    .unwrap();
    assert_eq!(read_back(&mut s), expected);
}

/// One gain value means a constant, which is what a client writing "halve
/// this" expects to be able to say.
#[test]
fn one_value_is_a_constant_gain_and_two_are_a_fade() {
    let mut s = session();
    let ramp = loaded(&mut s);
    send(
        &mut s,
        "/buffer_gain",
        vec![
            OscType::Int(0),
            OscType::Int(0),
            OscType::Int(-1), // to the end
            OscType::Float(0.0),
            OscType::Float(1.0),
        ],
    );
    s.settle_for(4);

    let mut expected = ramp;
    clausters_core::edit::gain(
        &mut expected,
        CHANNELS,
        0,
        FRAMES,
        clausters_core::edit::Fade::from_to(0.0, 1.0, clausters_core::envshape::SHAPE_LINEAR, 0.0),
    )
    .unwrap();
    let got = read_back(&mut s);
    assert_eq!(got, expected);
    assert_eq!(got[0], 0.0, "a fade in starts at silence");
    assert!(got[1] == 0.0 && got[2] > 0.0, "and rises from there");
}

#[test]
fn reverse_turns_the_span_around() {
    let mut s = session();
    let ramp = loaded(&mut s);
    send(
        &mut s,
        "/buffer_reverse",
        vec![OscType::Int(0), OscType::Int(0), OscType::Int(-1)],
    );
    s.settle_for(4);

    let mut expected = ramp;
    clausters_core::edit::reverse(&mut expected, CHANNELS, 0, FRAMES).unwrap();
    assert_eq!(read_back(&mut s), expected);
}

/// The subtle one: a batch of edits is submitted before any of them completes,
/// so each must build on what the *queue* last produced. Without the chain
/// every edit would start from the pre-batch contents and the last installed
/// would erase the rest — the defect `/buffer_setRange` already had to fix, and
/// the reason these jobs join the same chain.
#[test]
fn a_batch_of_edits_builds_on_each_other() {
    let mut s = session();
    let ramp = loaded(&mut s);
    // Fired back to back, no settle between them.
    send(
        &mut s,
        "/buffer_gain",
        vec![
            OscType::Int(0),
            OscType::Int(0),
            OscType::Int(4),
            OscType::Float(0.5),
        ],
    );
    send(
        &mut s,
        "/buffer_reverse",
        vec![OscType::Int(0), OscType::Int(0), OscType::Int(-1)],
    );
    send(
        &mut s,
        "/buffer_gain",
        vec![
            OscType::Int(0),
            OscType::Int(0),
            OscType::Int(-1),
            OscType::Float(2.0),
        ],
    );
    s.settle_for(8);

    let mut expected = ramp;
    clausters_core::edit::gain(
        &mut expected,
        CHANNELS,
        0,
        4,
        clausters_core::edit::Fade::constant(0.5),
    )
    .unwrap();
    clausters_core::edit::reverse(&mut expected, CHANNELS, 0, FRAMES).unwrap();
    clausters_core::edit::gain(
        &mut expected,
        CHANNELS,
        0,
        FRAMES,
        clausters_core::edit::Fade::constant(2.0),
    )
    .unwrap();
    assert_eq!(
        read_back(&mut s),
        expected,
        "three edits in flight must compose, not race"
    );
}

#[test]
fn a_span_past_the_end_fails_and_changes_nothing() {
    let mut s = session();
    let ramp = loaded(&mut s);
    let _ = fails(&mut s); // drain whatever the setup said
    send(
        &mut s,
        "/buffer_reverse",
        vec![
            OscType::Int(0),
            OscType::Int(FRAMES as i32 - 1),
            OscType::Int(4),
        ],
    );
    let reported = fails(&mut s);
    assert!(
        reported.iter().any(|f| f.contains("past the end")),
        "the caller is told rather than silently given less, got {reported:?}"
    );
    assert_eq!(read_back(&mut s), ramp, "and nothing was written");
}
