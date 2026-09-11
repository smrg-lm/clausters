//! **The multitrack's node system, heard rather than inspected.**
//!
//! `clausters_core::mixer` says what a piece *is* as nodes and groups, and a
//! def that is only read back as JSON proves nothing: what has to be true is
//! that the whole nest — piece, track, clip, reader — compiles, instantiates and
//! makes the sound the piece describes. So these render it offline and measure
//! what came out.
//!
//! What they check, in order: that a whole piece is one `/graph_new` and
//! everything after it is a slot added to what is already sounding; that a box
//! is heard where the transport says it is and nowhere else; that a port
//! written at any level reaches the control it names; and that a mono take on a
//! stereo track is *panned* rather than copied, which is the mixer bug worth a
//! measurement.

#![cfg(feature = "synth")]

use clausters::rosc::OscType;
use clausters::server::nrtsession::{NrtSession, SessionConfig};
use clausters_core::mixer;

const SR: f64 = 48_000.0;
const BLOCK: usize = 64;

fn session() -> NrtSession {
    NrtSession::open(&SessionConfig {
        sample_rate: SR,
        channels: 2,
        ..Default::default()
    })
    .expect("open")
}

fn send(s: &mut NrtSession, addr: &str, args: Vec<OscType>) {
    assert!(s.send_msg(addr, args).expect("encode"), "ring full");
}

/// Sends every def a piece of these widths needs, in the order the module says.
fn send_defs(s: &mut NrtSession, widths: &[(usize, usize)], master: usize) {
    let defs = mixer::defs_for(widths, master).expect("the widths are written");
    for def in &defs.synth {
        send(
            s,
            "/def_send",
            vec![
                OscType::String("synth".into()),
                OscType::String(def.to_string()),
            ],
        );
    }
    for def in &defs.graph {
        send(
            s,
            "/def_send",
            vec![
                OscType::String("graph".into()),
                OscType::String(def.to_string()),
            ],
        );
    }
    s.settle_for(8);
}

/// A mono buffer holding a constant, so what a strip did to it is arithmetic
/// rather than a spectrum.
fn dc(s: &mut NrtSession, index: i32, frames: usize, value: f32) {
    send(
        s,
        "/buffer_alloc",
        vec![
            OscType::Int(index),
            OscType::Int(frames as i32),
            OscType::Int(1),
        ],
    );
    s.settle_for(4);
    // A fill rather than a blob: the ring is not a place to put a take.
    send(
        s,
        "/buffer_fill",
        vec![
            OscType::Int(index),
            OscType::Int(0),
            OscType::Int(frames as i32),
            OscType::Float(value),
        ],
    );
    s.settle_for(4);
}

/// Every `/fail` that has arrived, as text -- what a silent render is asked
/// before it is believed.
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

/// Renders `blocks` blocks and answers the peak of each output channel.
fn peaks(s: &mut NrtSession, blocks: usize) -> (f32, f32) {
    let out = s
        .run_to_vec((blocks * BLOCK) as u64)
        .expect("the render ran");
    let mut left = 0.0f32;
    let mut right = 0.0f32;
    for frame in out.as_chunks::<2>().0 {
        left = left.max(frame[0].abs());
        right = right.max(frame[1].abs());
    }
    (left, right)
}

/// Builds a piece with one stereo track holding one mono clip whose one reader
/// plays buffer 0 from the transport's start, and answers the ids
/// `(piece, track, clip, reader)`.
fn one_box(s: &mut NrtSession, span_frames: f32, at_frames: f32) -> (i32, i32, i32, i32) {
    send(
        s,
        "/graph_new",
        vec![
            OscType::String(mixer::piece_name(2)),
            OscType::Int(900),
            OscType::Int(0),
            OscType::Int(0),
        ],
    );
    send(
        s,
        "/graph_addSlot",
        vec![
            OscType::Int(900),
            OscType::String(mixer::TRACK_SLOT.into()),
            OscType::Int(910),
        ],
    );
    send(
        s,
        "/graph_addSlot",
        vec![
            OscType::Int(910),
            OscType::String(mixer::clip_slot(1)),
            OscType::Int(920),
        ],
    );
    send(
        s,
        "/graph_addSlot",
        vec![
            OscType::Int(920),
            OscType::String(mixer::SOURCE_SLOT.into()),
            OscType::Int(930),
            OscType::String(mixer::BUF.into()),
            OscType::Float(0.0),
            OscType::String(mixer::AT.into()),
            OscType::Float(at_frames),
            OscType::String(mixer::SPAN.into()),
            OscType::Float(span_frames),
        ],
    );
    // **The piece's group is the transport's**, which is what makes the
    // readers' `TransportPos` the piece's own position rather than a number
    // that never moves -- and what makes play, stop and locate the engine's
    // rather than a client's arithmetic.
    send(s, "/transport_group", vec![OscType::Int(900)]);
    s.settle_for(8);
    (900, 910, 920, 930)
}

/// **A piece is one `/graph_new`, and everything else is added to what is
/// already sounding.** Four levels of nesting — piece, track, clip, reader —
/// and the sound comes out of the hardware bus at the end of them.
#[test]
fn a_whole_piece_is_one_graph_and_it_sounds() {
    let mut s = session();
    send_defs(&mut s, &[(1, 2)], 2);
    dc(&mut s, 0, 4800, 0.5);
    one_box(&mut s, 4800.0, 0.0);
    let refused = fails(&mut s);
    assert!(refused.is_empty(), "nothing was refused: {refused:?}");
    send(&mut s, "/transport_play", vec![]);
    s.settle_for(2);

    let (left, right) = peaks(&mut s, 8);
    assert!(left > 0.2, "the box is heard on the left: {left}");
    assert!(right > 0.2, "and on the right: {right}");
}

/// **A mono take on a stereo track is panned, not copied.** Centred, an
/// equal-power law puts about -3 dB on each side; the same signal twice would
/// put the whole of it on both, which is the classic mixer bug and 3 dB too
/// loud in the middle.
#[test]
fn a_mono_take_is_panned_into_the_stereo_track() {
    let mut s = session();
    send_defs(&mut s, &[(1, 2)], 2);
    dc(&mut s, 0, 4800, 1.0);
    one_box(&mut s, 4800.0, 0.0);
    send(&mut s, "/transport_play", vec![]);
    s.settle_for(2);

    let (left, right) = peaks(&mut s, 8);
    let centre = 1.0 / 2.0f32.sqrt();
    assert!(
        (left - centre).abs() < 0.05 && (right - centre).abs() < 0.05,
        "centred is -3 dB a side, not unity: left {left}, right {right}"
    );
    assert!(
        (left - right).abs() < 1e-3,
        "and the two sides are equal: {left} vs {right}"
    );
}

/// **A box is heard where the transport says it is, and nowhere else.** The
/// window is a gate read every block rather than a schedule, which is what
/// makes a locate cost no message at all.
#[test]
fn a_box_sounds_only_inside_its_own_window() {
    let mut s = session();
    send_defs(&mut s, &[(1, 2)], 2);
    dc(&mut s, 0, 48_000, 1.0);
    // The box begins four blocks in and lasts four.
    one_box(&mut s, (4 * BLOCK) as f32, (4 * BLOCK) as f32);
    send(&mut s, "/transport_play", vec![]);
    s.settle_for(2);

    // Block by block rather than in fixed thirds: what has to be true is the
    // *shape* -- silence, then a run of sound, then silence -- and how many
    // blocks the commands took to settle is not part of it.
    let heard: Vec<bool> = (0..16).map(|_| peaks(&mut s, 1).0 > 0.2).collect();
    let first = heard.iter().position(|&h| h).expect("it is heard at all");
    let last = heard.iter().rposition(|&h| h).expect("it is heard at all");
    assert!(
        heard[first..=last].iter().all(|&h| h),
        "one unbroken run rather than a stutter: {heard:?}"
    );
    assert!(first > 0, "silent before it starts: {heard:?}");
    assert!(
        last < heard.len() - 1,
        "and silent again after it: {heard:?}"
    );
    assert_eq!(
        last - first + 1,
        4,
        "four blocks long, which is the span it was given: {heard:?}"
    );
}

/// **A port written at any level reaches the control it names.** The track's
/// mute and the piece's gain are the same word on three different strips, which
/// is what makes an automation's target resolvable at all.
#[test]
fn a_port_at_any_level_reaches_the_strip_it_names() {
    let mut s = session();
    send_defs(&mut s, &[(1, 2)], 2);
    dc(&mut s, 0, 48_000, 1.0);
    let (piece, track, clip, _reader) = one_box(&mut s, 48_000.0, 0.0);
    send(&mut s, "/transport_play", vec![]);
    s.settle_for(2);
    assert!(peaks(&mut s, 8).0 > 0.2, "it starts audible");

    for (node, port) in [
        (track, mixer::MUTE),
        (clip, mixer::MUTE),
        (piece, mixer::MUTE),
    ] {
        send(
            &mut s,
            "/node_set",
            vec![
                OscType::Int(node),
                OscType::String(port.into()),
                OscType::Float(1.0),
            ],
        );
        // Past the fader's lag.
        s.settle_for(2);
        let _settling = peaks(&mut s, 40);
        let silent = peaks(&mut s, 8);
        assert!(
            silent.0 < 1e-3,
            "{port} on node {node} silenced it: {silent:?}"
        );
        send(
            &mut s,
            "/node_set",
            vec![
                OscType::Int(node),
                OscType::String(port.into()),
                OscType::Float(0.0),
            ],
        );
        s.settle_for(2);
        let _settling = peaks(&mut s, 40);
        assert!(
            peaks(&mut s, 8).0 > 0.2,
            "and unmuting it brought the box back ({port} on {node})"
        );
    }
}
