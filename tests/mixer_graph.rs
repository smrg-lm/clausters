//! **The multitrack's node system, heard rather than inspected.**
//!
//! `clausters_core::mixer` says what a multitrack *is* as nodes and groups, and a
//! def that is only read back as JSON proves nothing: what has to be true is
//! that the whole nest -- multitrack, track, clip, reader -- compiles, instantiates and
//! makes the sound the multitrack describes. So these render it offline and measure
//! what came out.
//!
//! What they check, in order: that a whole multitrack is one `/graph_new` and
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

/// The tracks' group [`one_box`] makes: the slot of the multitrack the tracks
/// are added to.
const TRACKS: i32 = 905;

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

/// Sends every def a multitrack of these widths needs, in the order the module says.
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

/// Builds a multitrack with one stereo track holding one mono clip whose one reader
/// plays buffer 0 from the transport's start, and answers the ids
/// `(multitrack, track, clip, reader)`.
fn one_box(s: &mut NrtSession, span_frames: f32, at_frames: f32) -> (i32, i32, i32, i32) {
    one_box_on(s, 2, 2, span_frames, at_frames)
}

/// [`one_box`] with the widths said: a master of `master` channels and one
/// track of `track`, which goes in the slot its own width names.
fn one_box_on(
    s: &mut NrtSession,
    master: usize,
    track: usize,
    span_frames: f32,
    at_frames: f32,
) -> (i32, i32, i32, i32) {
    send(
        s,
        "/graph_new",
        vec![
            OscType::String(mixer::multitrack_name(master)),
            OscType::Int(900),
            OscType::Int(0),
            OscType::Int(0),
        ],
    );
    // **The tracks' group**: the slot of the multitrack the tracks go in, and
    // the one the transport governs.
    send(
        s,
        "/graph_addSlot",
        vec![
            OscType::Int(900),
            OscType::String(mixer::TRANSPORT_SLOT.into()),
            OscType::Int(TRACKS),
        ],
    );
    send(
        s,
        "/graph_addSlot",
        vec![
            OscType::Int(TRACKS),
            OscType::String(mixer::track_slot(track)),
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
    // **The tracks' group is the transport's**, which is what makes the
    // readers' `TransportPos` the multitrack's own position rather than a number
    // that never moves -- and what makes play, stop and locate the engine's
    // rather than a client's arithmetic. The master around it is not governed.
    send(
        s,
        "/transport_group",
        vec![OscType::Int(0), OscType::Int(TRACKS)],
    );
    s.settle_for(8);
    (900, 910, 920, 930)
}

/// Adds a meter to a strip's meter slot, writing channel 0 to `bus` and
/// channel 1 to the one after it.
fn meter(s: &mut NrtSession, instance: i32, id: i32, bus: i32, hold: f32) {
    send(
        s,
        "/graph_addSlot",
        vec![
            OscType::Int(instance),
            OscType::String(mixer::METER_SLOT.into()),
            OscType::Int(id),
            OscType::String(mixer::METER_OUT0.into()),
            OscType::Float(bus as f32),
            OscType::String(mixer::METER_OUT1.into()),
            OscType::Float((bus + 1) as f32),
            OscType::String(mixer::METER_HOLD_PORT.into()),
            OscType::Float(hold),
        ],
    );
    s.settle_for(4);
}

/// **A multitrack is one `/graph_new`, and everything else is added to what is
/// already sounding.** Four levels of nesting -- multitrack, track, clip, reader --
/// and the sound comes out of the hardware bus at the end of them.
#[test]
fn a_whole_multitrack_is_one_graph_and_it_sounds() {
    let mut s = session();
    send_defs(&mut s, &[(1, 2)], 2);
    dc(&mut s, 0, 4800, 0.5);
    one_box(&mut s, 4800.0, 0.0);
    let refused = fails(&mut s);
    assert!(refused.is_empty(), "nothing was refused: {refused:?}");
    send(&mut s, "/transport_play", vec![OscType::Int(0)]);
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
    send(&mut s, "/transport_play", vec![OscType::Int(0)]);
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

/// **A mono track is one channel to its meter, and panned on its way out.**
/// The document says how wide a track is and the master's width does not
/// change it: the take is one channel through the track's fader, one channel
/// in its meter at the level the fader left it, and the pan is the last thing
/// the track does, into a master of two.
#[test]
fn a_mono_track_is_one_channel_until_it_meets_the_master() {
    let mut s = session();
    send_defs(&mut s, &[(1, 1)], 2);
    dc(&mut s, 0, 48_000, 1.0);
    let (_multitrack, track, ..) = one_box_on(&mut s, 2, 1, 48_000.0, 0.0);
    // Its level and nothing beside it: the meter of a mono track writes one
    // bus, and the next one is somebody else's.
    send(
        &mut s,
        "/graph_addSlot",
        vec![
            OscType::Int(track),
            OscType::String(mixer::METER_SLOT.into()),
            OscType::Int(960),
            OscType::String(mixer::METER_OUT0.into()),
            OscType::Float(110.0),
        ],
    );
    s.settle_for(4);
    let refused = fails(&mut s);
    assert!(refused.is_empty(), "nothing was refused: {refused:?}");
    send(&mut s, "/transport_play", vec![OscType::Int(0)]);
    s.settle_for(2);

    let (left, right) = peaks(&mut s, 8);
    let centre = 1.0 / 2.0f32.sqrt();
    assert!(
        (left - centre).abs() < 0.05 && (right - centre).abs() < 0.05,
        "centred is -3 dB a side: left {left}, right {right}"
    );
    let level = bus_value(&mut s, 110);
    assert!(
        (level - 1.0).abs() < 0.05,
        "the meter reads the one channel after the fader, before the pan: {level}"
    );
    assert_eq!(bus_value(&mut s, 111), 0.0, "and writes no second channel");

    // The track's pan is the pan into the master: all of it on one side.
    send(
        &mut s,
        "/node_set",
        vec![
            OscType::Int(track),
            OscType::String(mixer::PAN.into()),
            OscType::Float(-1.0),
        ],
    );
    s.settle_for(2);
    let _settling = peaks(&mut s, 40);
    let (left, right) = peaks(&mut s, 8);
    assert!((left - 1.0).abs() < 0.05, "hard left is unity: {left}");
    assert!(right < 1e-3, "and nothing on the right: {right}");
    let level = bus_value(&mut s, 110);
    assert!(
        (level - 1.0).abs() < 0.05,
        "the pan is after the meter, so the level did not move: {level}"
    );

    // And its gain is the fader before both.
    send(
        &mut s,
        "/node_set",
        vec![
            OscType::Int(track),
            OscType::String(mixer::GAIN.into()),
            OscType::Float(0.5),
        ],
    );
    s.settle_for(2);
    let _settling = peaks(&mut s, 40);
    let (left, _) = peaks(&mut s, 8);
    assert!(
        (left - 0.5).abs() < 0.05,
        "half the fader, half the sound: {left}"
    );
    // The meter falls at its own rate rather than jumping, so what it shows
    // by now is on its way down from unity and not yet past the new level.
    let level = bus_value(&mut s, 110);
    assert!(
        (0.5 - 0.05..0.95).contains(&level),
        "and the meter is after the fader, so it is falling to half: {level}"
    );
}

/// **A stereo track in a mono multitrack keeps its two sides to its meter and
/// is summed on its way out.** The other direction of the same rule.
#[test]
fn a_stereo_track_is_summed_into_a_mono_master() {
    let mut s = session();
    send_defs(&mut s, &[(1, 2)], 1);
    dc(&mut s, 0, 48_000, 0.5);
    let (_multitrack, track, ..) = one_box_on(&mut s, 1, 2, 48_000.0, 0.0);
    meter(&mut s, track, 960, 110, 0.0);
    let refused = fails(&mut s);
    assert!(refused.is_empty(), "nothing was refused: {refused:?}");
    send(&mut s, "/transport_play", vec![OscType::Int(0)]);
    s.settle_for(2);

    // The mono take is panned into the stereo track at -3 dB a side, and the
    // master is the sum of the two.
    let (left, right) = peaks(&mut s, 8);
    let side = 0.5 / 2.0f32.sqrt();
    assert!(
        (left - 2.0 * side).abs() < 0.05,
        "the master is both sides summed: {left}"
    );
    assert!(right < 1e-3, "a mono master writes one channel: {right}");
    let (l, r) = (bus_value(&mut s, 110), bus_value(&mut s, 111));
    assert!(
        (l - side).abs() < 0.05 && (r - side).abs() < 0.05,
        "the track's meter has its two sides: {l}, {r}"
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
    send(&mut s, "/transport_play", vec![OscType::Int(0)]);
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
/// mute and the multitrack's gain are the same word on three different strips, which
/// is what makes an automation's target resolvable at all.
#[test]
fn a_port_at_any_level_reaches_the_strip_it_names() {
    let mut s = session();
    send_defs(&mut s, &[(1, 2)], 2);
    dc(&mut s, 0, 48_000, 1.0);
    let (multitrack, track, clip, _reader) = one_box(&mut s, 48_000.0, 0.0);
    send(&mut s, "/transport_play", vec![OscType::Int(0)]);
    s.settle_for(2);
    assert!(peaks(&mut s, 8).0 > 0.2, "it starts audible");

    for (node, port) in [
        (track, mixer::MUTE),
        (clip, mixer::MUTE),
        (multitrack, mixer::MUTE),
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

/// **A box moved to another track sounds through that track, and keeps what
/// was on it.** `/graph_moveSlot` re-wires the clip to the new track's mix bus
/// rather than making it again, so a box dragged onto a muted track goes
/// silent, dragged back it is heard again -- and a port mapped onto a control
/// bus before the move is still driven after it.
#[test]
fn a_moved_box_sounds_through_its_new_track_and_keeps_its_map() {
    let mut s = session();
    send_defs(&mut s, &[(1, 2)], 2);
    dc(&mut s, 0, 48_000, 1.0);
    let (_multitrack, track, clip, _reader) = one_box(&mut s, 48_000.0, 0.0);
    let other = 911;
    send(
        &mut s,
        "/graph_addSlot",
        vec![
            OscType::Int(TRACKS),
            OscType::String(mixer::track_slot(2)),
            OscType::Int(other),
            OscType::String(mixer::MUTE.into()),
            OscType::Float(1.0),
        ],
    );
    // The clip's gain from a control bus, the way a curve drives it.
    let bus = 40;
    send(
        &mut s,
        "/bus_set",
        vec![OscType::Int(bus), OscType::Float(0.5)],
    );
    send(
        &mut s,
        "/graph_map",
        vec![
            OscType::Int(clip),
            OscType::String(mixer::GAIN.into()),
            OscType::Int(bus),
        ],
    );
    send(&mut s, "/transport_play", vec![OscType::Int(0)]);
    s.settle_for(2);
    let _settling = peaks(&mut s, 40);
    let before = peaks(&mut s, 8).0;
    assert!(before > 0.1, "it starts audible: {before}");

    let heard_after = |s: &mut NrtSession, to: i32| {
        send(
            s,
            "/graph_moveSlot",
            vec![OscType::Int(clip), OscType::Int(to)],
        );
        s.settle_for(2);
        let _settling = peaks(s, 40);
        peaks(s, 8).0
    };
    let muted = heard_after(&mut s, other);
    assert!(muted < 1e-3, "on the muted track it is silent: {muted}");
    assert!(fails(&mut s).is_empty(), "{:?}", fails(&mut s));
    let back = heard_after(&mut s, track);
    assert!(
        (back - before).abs() < 1e-3,
        "back on its track it is what it was, the mapped gain included: {back} vs {before}"
    );

    // Still the bus's to drive: the map went with the node.
    send(
        &mut s,
        "/bus_set",
        vec![OscType::Int(bus), OscType::Float(0.25)],
    );
    s.settle_for(2);
    let _settling = peaks(&mut s, 40);
    let halved = peaks(&mut s, 8).0;
    assert!(
        (halved - before / 2.0).abs() < 0.02,
        "the bus still drives the moved clip's gain: {halved} vs {before}"
    );
}

/// **A curve drives a port, and the port is a control of a node three levels
/// down.** The whole of what a multitrack's automation is: a table read at the
/// transport's own position, written to a control bus, mapped onto whatever the
/// curve names -- so a locate costs no message and the member ids stay private.
#[test]
fn a_curve_on_a_bus_drives_a_port() {
    let mut s = session();
    send_defs(&mut s, &[(1, 2)], 2);
    dc(&mut s, 0, 48_000, 1.0);
    let (_multitrack, track, _clip, _reader) = one_box(&mut s, 48_000.0, 0.0);

    // A table that rises from silence to unity over eight blocks, read one
    // sample a block.
    let step = BLOCK as f32;
    let blocks = 8;
    send(
        &mut s,
        "/buffer_alloc",
        vec![OscType::Int(1), OscType::Int(blocks + 1), OscType::Int(1)],
    );
    s.settle_for(4);
    for i in 0..=blocks {
        send(
            &mut s,
            "/buffer_set",
            vec![
                OscType::Int(1),
                OscType::Int(i),
                OscType::Float(i as f32 / blocks as f32),
            ],
        );
    }
    s.settle_for(4);

    // The curve node, and the track's gain mapped to what it writes.
    let bus = 100;
    send(
        &mut s,
        "/synth_new",
        vec![
            OscType::String(mixer::curve_name()),
            OscType::Int(950),
            OscType::Int(0),
            OscType::Int(0),
            OscType::String("out".into()),
            OscType::Float(bus as f32),
            OscType::String(mixer::BUF.into()),
            OscType::Float(1.0),
            OscType::String(mixer::AT.into()),
            OscType::Float(0.0),
            OscType::String("step".into()),
            OscType::Float(step),
        ],
    );
    send(
        &mut s,
        "/graph_map",
        vec![
            OscType::Int(track),
            OscType::String(mixer::GAIN.into()),
            OscType::Int(bus),
        ],
    );
    let refused = fails(&mut s);
    assert!(refused.is_empty(), "nothing was refused: {refused:?}");

    send(&mut s, "/transport_play", vec![OscType::Int(0)]);
    s.settle_for(2);
    let heard: Vec<f32> = (0..blocks + 2).map(|_| peaks(&mut s, 1).0).collect();
    assert!(
        heard[0] < heard[heard.len() - 1],
        "the curve opened the fader: {heard:?}"
    );
    assert!(
        heard.windows(2).filter(|w| w[0] > w[1] + 1e-3).count() <= 1,
        "and it rose rather than wandering: {heard:?}"
    );

    // **A set takes the port back**, which is the rule the other half of this
    // rests on: setting a mapped control is how a hand reclaims a fader, and
    // therefore how a client that re-sends a track's gain on every edit
    // silences its own automation without touching the curve at all. The
    // client's answer is not to send a value for a port a curve drives
    // (`hand_ports`); this is the behaviour that makes that necessary.
    send(
        &mut s,
        "/node_set",
        vec![
            OscType::Int(track),
            OscType::String(mixer::GAIN.into()),
            OscType::Float(0.0),
        ],
    );
    s.settle_for(2);
    let _settling = peaks(&mut s, 40);
    assert!(
        peaks(&mut s, 4).0 < 1e-3,
        "the set holds against a curve that is still writing the bus"
    );
    // Mapped again, the curve drives it again -- the set took the port, it did
    // not break the curve.
    send(
        &mut s,
        "/graph_map",
        vec![
            OscType::Int(track),
            OscType::String(mixer::GAIN.into()),
            OscType::Int(bus),
        ],
    );
    s.settle_for(2);
    let _resettling = peaks(&mut s, 40);
    assert!(
        peaks(&mut s, 4).0 > 1e-3,
        "and mapping it again hands it back to the curve"
    );

    // Unmapping gives the port back, and what was last on the bus does not
    // keep driving it.
    send(
        &mut s,
        "/graph_map",
        vec![
            OscType::Int(track),
            OscType::String(mixer::GAIN.into()),
            OscType::Int(-1),
        ],
    );
    send(
        &mut s,
        "/node_set",
        vec![
            OscType::Int(track),
            OscType::String(mixer::GAIN.into()),
            OscType::Float(0.0),
        ],
    );
    s.settle_for(2);
    let _settling = peaks(&mut s, 40);
    assert!(
        peaks(&mut s, 4).0 < 1e-3,
        "the hand has the fader back once the curve is unmapped"
    );
}

/// **A meter reads a level the way a person does**: up at once, down at a
/// declared rate, and a peak that stays put long enough to be seen.
///
/// Rendered rather than unit-tested, because what has to be true is that the
/// def compiles, the UGen runs at the engine's own rate and the value lands on
/// a control bus a host can read as a range.
#[test]
fn a_meter_writes_a_readable_level_to_a_control_bus() {
    let mut s = session();
    send_defs(&mut s, &[(1, 2)], 2);
    dc(&mut s, 0, 48_000, 1.0);
    one_box(&mut s, (8 * BLOCK) as f32, 0.0);

    // The master's own level, and its mark beside it: two instances of one
    // slot, which is what makes the mark cost nothing when nobody looks.
    meter(&mut s, 900, 960, 110, 0.0);
    meter(&mut s, 900, 961, 112, mixer::METER_HOLD);
    let refused = fails(&mut s);
    assert!(refused.is_empty(), "nothing was refused: {refused:?}");

    send(&mut s, "/transport_play", vec![OscType::Int(0)]);
    s.settle_for(2);
    let _sounding = peaks(&mut s, 6);
    let loud = bus_value(&mut s, 110);
    assert!(loud > 0.2, "the meter read the level: {loud}");
    assert!(
        bus_value(&mut s, 111) > 0.2,
        "and the second channel has a bus of its own"
    );

    // Past the box, the level falls and the mark does not.
    let _silence = peaks(&mut s, 12);
    let fallen = bus_value(&mut s, 110);
    let held = bus_value(&mut s, 112);
    assert!(
        fallen < loud,
        "it falls once the box is over: {fallen} < {loud}"
    );
    assert!(
        held >= fallen,
        "and the mark is still up there: {held} >= {fallen}"
    );

    // **Every channel is metered, not just the first.** Measured while the
    // level is falling, because that is where a channel reading its raw
    // *signal* instead of its meter is told from one that is metered: the
    // signal is already zero and a meter is on its way down. A centred mono
    // take is the same amplitude on both sides, so the two buses agree to the
    // sample.
    let fallen_right = bus_value(&mut s, 111);
    assert!(
        (fallen_right - fallen).abs() < 1e-6,
        "the right channel is a meter and falls with the left:          {fallen_right} against {fallen}"
    );
    assert!(
        (bus_value(&mut s, 113) - held).abs() < 1e-6,
        "and so is its mark"
    );
}

/// **A track's meter reads that track and not the multitrack.** Every track writes
/// into the master's mix bus, so a meter there would read the sum and call it
/// the track -- which is why a strip writes its own `post` bus and a send
/// carries it the rest of the way.
#[test]
fn a_track_is_metered_on_its_own_output() {
    let mut s = session();
    send_defs(&mut s, &[(1, 2)], 2);
    dc(&mut s, 0, 48_000, 1.0);
    let (multitrack, loud_track, ..) = one_box(&mut s, (16 * BLOCK) as f32, 0.0);

    // A second track with nothing on it, beside the one that sounds.
    let quiet_track = 911;
    send(
        &mut s,
        "/graph_addSlot",
        vec![
            OscType::Int(TRACKS),
            OscType::String(mixer::track_slot(2)),
            OscType::Int(quiet_track),
        ],
    );
    meter(&mut s, loud_track, 970, 120, 0.0);
    meter(&mut s, quiet_track, 971, 122, 0.0);
    meter(&mut s, multitrack, 972, 124, 0.0);
    let refused = fails(&mut s);
    assert!(refused.is_empty(), "nothing was refused: {refused:?}");

    send(&mut s, "/transport_play", vec![OscType::Int(0)]);
    s.settle_for(2);
    let _sounding = peaks(&mut s, 8);
    assert!(
        bus_value(&mut s, 120) > 0.2,
        "the track that sounds reads its own level"
    );
    assert!(
        bus_value(&mut s, 122) < 1e-3,
        "the track that does not sound reads nothing, not the multitrack"
    );
    assert!(
        bus_value(&mut s, 124) > 0.2,
        "and the master reads the sum, which is its own output"
    );
}

/// **A track's output is a send**, so turning it down turns the track down in
/// the master without touching the fader the automation writes.
#[test]
fn a_strips_output_is_a_send_with_a_gain() {
    let mut s = session();
    send_defs(&mut s, &[(1, 2)], 2);
    dc(&mut s, 0, 48_000, 1.0);
    let (_, track, ..) = one_box(&mut s, (16 * BLOCK) as f32, 0.0);
    meter(&mut s, track, 980, 130, 0.0);
    send(&mut s, "/transport_play", vec![OscType::Int(0)]);
    s.settle_for(2);
    let (open, open_right) = peaks(&mut s, 6);
    let metered = bus_value(&mut s, 130);

    send(
        &mut s,
        "/node_set",
        vec![
            OscType::Int(track),
            OscType::String(mixer::SEND_GAIN.into()),
            OscType::Float(0.0),
        ],
    );
    s.settle_for(2);
    let _settling = peaks(&mut s, 8);
    let (shut, shut_right) = peaks(&mut s, 4);
    assert!(open > 0.2 && shut < 1e-3, "shut: {open} -> {shut}");
    // **Both channels**, because a gain that reached only the first would be a
    // right channel no fader, no mute and no send could ever quiet -- and at
    // the default gain of one it would sound exactly right.
    assert!(
        open_right > 0.2 && shut_right < 1e-3,
        "and the right channel went through the same gain:          {open_right} -> {shut_right}"
    );
    assert!(
        bus_value(&mut s, 130) > 0.2 * metered,
        "and the track still reads its own level, which is before the send"
    );
}

/// One control bus's value, over the wire.
fn bus_value(s: &mut NrtSession, index: i32) -> f32 {
    send(s, "/bus_get", vec![OscType::Int(index)]);
    let mut buf = vec![0u8; 1 << 16];
    for _ in 0..64 {
        while let Some(len) = s.poll_into(&mut buf) {
            if let Ok(clausters::rosc::OscPacket::Message(m)) =
                clausters::osc::decode_packet(&buf[..len])
                && m.addr == "/bus_get.reply"
                && let Some(OscType::Float(v)) = m.args.last()
            {
                return *v;
            }
        }
        s.settle();
    }
    panic!("the bus never answered")
}

/// Where the click is **on a transport given no ramp**: the step the output
/// takes at a stop and at a play from a cue inside a take.
///
/// A measurement rather than an assertion, and the number it prints is what
/// `/transport_fade` takes off -- which is asserted, with a ramp set, in
/// `a_stop_fades_the_master_and_leaves_its_meter_to_fall`. No ramp is the
/// server's default, so this is still what a client that sets none hears:
/// `cargo test --test mixer_graph where_the_transport -- --ignored --nocapture`
#[test]
#[ignore]
fn where_the_transport_clicks() {
    let mut s = session();
    send_defs(&mut s, &[(1, 2)], 2);
    // A take that is loud everywhere, so a cut anywhere is a step of that
    // size: a constant is the worst case and the clearest one.
    dc(&mut s, 0, 48_000, 0.8);
    one_box(&mut s, 48_000.0, 0.0);
    send(&mut s, "/transport_play", vec![OscType::Int(0)]);
    s.settle_for(2);
    let _rolling = peaks(&mut s, 8);

    /// The largest sample-to-sample jump in the left channel of a run, read
    /// **across** the block seams -- which is where a transport edge lands.
    fn step(out: &[f32]) -> f32 {
        out.as_chunks::<2>()
            .0
            .windows(2)
            .map(|w| (w[1][0] - w[0][0]).abs())
            .fold(0.0f32, f32::max)
    }
    let run =
        |s: &mut NrtSession, blocks: usize| s.run_to_vec((blocks * BLOCK) as u64).expect("ran");

    let rolling = run(&mut s, 4);
    send(&mut s, "/transport_stop", vec![OscType::Int(0)]);
    s.settle_for(2);
    let mut stopping = rolling[rolling.len() - 2..].to_vec();
    stopping.extend(run(&mut s, 4));
    send(&mut s, "/transport_play", vec![OscType::Int(0)]);
    s.settle_for(2);
    let mut starting = stopping[stopping.len() - 2..].to_vec();
    starting.extend(run(&mut s, 4));

    println!("the largest step while it rolls:  {:.4}", step(&rolling));
    println!("the largest step at the stop:     {:.4}", step(&stopping));
    println!("the largest step at the play:     {:.4}", step(&starting));
    println!(
        "\nA step the size of what was sounding is a click. With no ramp set \
         the transport freezes the subtree and thaws it, so a stop and a play \
         are square edges of whatever the take happened to be at -- which is \
         what /transport_fade takes off, and what the readers cannot do for \
         themselves: a frozen node gets no time."
    );
}

/// **A curve's first block is the value it starts at.** A box whose clip
/// envelope begins at zero is silent where it begins, and anything audible
/// there is a click nobody drew.
///
/// The control bus carries one number a block and whoever reads it holds that
/// number for the whole block, so writing the block's *last* sample handed the
/// strip a value a block early: an envelope that says zero at the box's start
/// put out the value a block after it, and a multitrack thawed from a stop has no
/// smoothing left to hide it with.
#[test]
fn a_curve_starts_where_it_says_it_starts() {
    let mut s = session();
    send_defs(&mut s, &[(1, 2)], 2);
    dc(&mut s, 0, 48_000, 0.8);
    let (_multitrack, _track, clip, _reader) = one_box(&mut s, 48_000.0, 0.0);

    // The envelope: zero at the box's start, rising to unity over eight blocks,
    // one sample a block -- the clip envelope an editor draws.
    let step = BLOCK as f32;
    let blocks = 8;
    send(
        &mut s,
        "/buffer_alloc",
        vec![OscType::Int(1), OscType::Int(blocks + 1), OscType::Int(1)],
    );
    s.settle_for(4);
    for i in 0..=blocks {
        send(
            &mut s,
            "/buffer_set",
            vec![
                OscType::Int(1),
                OscType::Int(i),
                OscType::Float(i as f32 / blocks as f32),
            ],
        );
    }
    s.settle_for(4);
    let bus = 100;
    send(
        &mut s,
        "/synth_new",
        vec![
            OscType::String(mixer::curve_name()),
            OscType::Int(950),
            OscType::Int(0),
            OscType::Int(0),
            OscType::String("out".into()),
            OscType::Float(bus as f32),
            OscType::String(mixer::BUF.into()),
            OscType::Float(1.0),
            OscType::String(mixer::AT.into()),
            OscType::Float(0.0),
            OscType::String("step".into()),
            OscType::Float(step),
        ],
    );
    send(
        &mut s,
        "/graph_map",
        vec![
            OscType::Int(clip),
            OscType::String(mixer::GAIN.into()),
            OscType::Int(bus),
        ],
    );
    s.settle_for(4);
    let refused = fails(&mut s);
    assert!(refused.is_empty(), "nothing was refused: {refused:?}");

    send(&mut s, "/transport_play", vec![OscType::Int(0)]);
    s.settle_for(2);
    let first = s.run_to_vec(BLOCK as u64).expect("ran");
    let peak = first.iter().step_by(2).fold(0.0f32, |m, &x| m.max(x.abs()));
    assert!(
        peak < 1e-6,
        "the box is silent where its envelope says zero: {peak}"
    );
    let second = s.run_to_vec(BLOCK as u64).expect("ran");
    assert!(
        second.iter().step_by(2).any(|x| x.abs() > 1e-4),
        "and it comes up right after"
    );
}

/// **A stop is not a pause of the numbers.** The transport freezes the multitrack's
/// subtree, so every smoother in it is starved of time and keeps the value it
/// had when the music stopped -- while the curve that drives it, which is not
/// in that subtree, goes on writing wherever the position now is. Play again
/// and the strip glides from the old value to the new one over the lag, which
/// is a burst of whatever the box holds at a level nothing asked for: the click
/// at a box that begins in silence.
#[test]
fn a_thawed_strip_does_not_glide_down_from_where_it_stopped() {
    let mut s = session();
    send_defs(&mut s, &[(1, 2)], 2);
    dc(&mut s, 0, 48_000, 0.8);
    let (_multitrack, _track, clip, _reader) = one_box(&mut s, 48_000.0, 0.0);

    // A curve flat at unity, then flat at zero: the envelope's first point
    // dragged to the floor while the transport stands still.
    send(
        &mut s,
        "/buffer_alloc",
        vec![OscType::Int(1), OscType::Int(4), OscType::Int(1)],
    );
    s.settle_for(4);
    for i in 0..4 {
        send(
            &mut s,
            "/buffer_set",
            vec![OscType::Int(1), OscType::Int(i), OscType::Float(1.0)],
        );
    }
    s.settle_for(4);
    let bus = 100;
    send(
        &mut s,
        "/synth_new",
        vec![
            OscType::String(mixer::curve_name()),
            OscType::Int(950),
            OscType::Int(0),
            OscType::Int(0),
            OscType::String("out".into()),
            OscType::Float(bus as f32),
            OscType::String(mixer::BUF.into()),
            OscType::Float(1.0),
            OscType::String(mixer::AT.into()),
            OscType::Float(0.0),
            OscType::String("step".into()),
            OscType::Float(BLOCK as f32),
        ],
    );
    send(
        &mut s,
        "/graph_map",
        vec![
            OscType::Int(clip),
            OscType::String(mixer::GAIN.into()),
            OscType::Int(bus),
        ],
    );
    s.settle_for(4);

    // It played once at unity.
    send(&mut s, "/transport_play", vec![OscType::Int(0)]);
    s.settle_for(2);
    let (heard, _) = peaks(&mut s, 8);
    assert!(heard > 0.5, "it played at unity first: {heard}");

    // Stopped, the envelope's first point goes to zero, and the multitrack rewinds.
    send(&mut s, "/transport_stop", vec![OscType::Int(0)]);
    s.settle_for(2);
    for i in 0..4 {
        send(
            &mut s,
            "/buffer_set",
            vec![OscType::Int(1), OscType::Int(i), OscType::Float(0.0)],
        );
    }
    send(
        &mut s,
        "/transport_locate",
        vec![OscType::Int(0), OscType::Float(0.0)],
    );
    s.settle_for(4);

    send(&mut s, "/transport_play", vec![OscType::Int(0)]);
    s.settle_for(2);
    let (left, _) = peaks(&mut s, 4);
    assert!(
        left < 1e-4,
        "a box whose envelope says zero is silent from the first sample: {left}"
    );
}

/// And the same is true of what the meter says. A held peak means "the loudest
/// thing lately"; lately ended when the transport did, so a meter thawed with
/// the last pass's mark still up draws a level the multitrack has not played a
/// sample of.
#[test]
fn a_thawed_meter_does_not_report_the_pass_before_it() {
    let mut s = session();
    send_defs(&mut s, &[(1, 2)], 2);
    dc(&mut s, 0, 48_000, 1.0);
    // The box is one block long, so a rewind puts the multitrack in front of
    // silence with the loud pass still in the meter's memory.
    let (_multitrack, track, ..) = one_box(&mut s, BLOCK as f32, 0.0);
    meter(&mut s, track, 970, 120, mixer::METER_HOLD);
    let refused = fails(&mut s);
    assert!(refused.is_empty(), "nothing was refused: {refused:?}");

    send(&mut s, "/transport_play", vec![OscType::Int(0)]);
    s.settle_for(2);
    let _sounding = peaks(&mut s, 8);
    assert!(bus_value(&mut s, 120) > 0.2, "it read the loud pass");

    send(&mut s, "/transport_stop", vec![OscType::Int(0)]);
    s.settle_for(2);
    // Past the box, where there is nothing to hear.
    send(
        &mut s,
        "/transport_locate",
        vec![
            OscType::Int(0),
            OscType::Float((8 * BLOCK) as f32 / SR as f32),
        ],
    );
    s.settle_for(2);
    send(&mut s, "/transport_play", vec![OscType::Int(0)]);
    s.settle_for(2);
    let _silent = peaks(&mut s, 4);
    assert!(
        bus_value(&mut s, 120) < 1e-3,
        "and it reads the silence it is playing now: {}",
        bus_value(&mut s, 120)
    );
}

/// **A stop freezes the tracks and leaves the master running**: with the
/// transport's ramp the hardware output fades to zero rather than stepping, a
/// track's meter closes on the way down and reads exactly zero once frozen,
/// and the master's meter -- outside the governed group -- falls rather than
/// holding what it last saw.
#[test]
fn a_stop_fades_the_master_and_leaves_its_meter_to_fall() {
    let mut s = session();
    send_defs(&mut s, &[(1, 2)], 2);
    dc(&mut s, 0, 48_000, 0.5);
    let (multitrack, track, ..) = one_box(&mut s, 48_000.0, 0.0);
    meter(&mut s, track, 970, 120, 0.0);
    meter(&mut s, multitrack, 972, 124, 0.0);
    send(
        &mut s,
        "/transport_fade",
        vec![OscType::Int(0), OscType::Long(240)],
    );
    send(&mut s, "/transport_play", vec![OscType::Int(0)]);
    s.settle_for(2);
    let before = s.run_to_vec((16 * BLOCK) as u64).expect("the render ran");
    assert!(bus_value(&mut s, 124) > 0.2, "the master reads the sum");

    send(&mut s, "/transport_stop", vec![OscType::Int(0)]);
    let refused = fails(&mut s);
    assert!(refused.is_empty(), "nothing was refused: {refused:?}");
    let after = s.run_to_vec((16 * BLOCK) as u64).expect("the render ran");
    let left: Vec<f32> = before
        .as_chunks::<2>()
        .0
        .iter()
        .chain(after.as_chunks::<2>().0)
        .map(|f| f[0])
        .collect();
    let step = left
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(
        step < 0.01,
        "the stop is a ramp at the hardware, not a step: {step}"
    );
    assert!(
        left[left.len() - BLOCK..].iter().all(|x| *x == 0.0),
        "and then silence: {:?}",
        &left[left.len() - 4..]
    );
    assert_eq!(
        bus_value(&mut s, 120),
        0.0,
        "a frozen track's meter reads zero, not the level it froze on"
    );
    let _ = s.run_to_vec((1_500 * BLOCK) as u64).expect("two seconds");
    assert!(
        bus_value(&mut s, 124) < 0.02,
        "the master's meter fell, since nothing froze it"
    );
}

/// **A stereo take keeps its two sides.** Its readers are one per channel of
/// the source, and each lands on its own channel of the clip: a take with
/// sound on the right alone is heard on the right alone.
#[test]
fn a_stereo_take_keeps_its_two_sides() {
    let mut s = session();
    send_defs(&mut s, &[(2, 2)], 2);
    send(
        &mut s,
        "/buffer_alloc",
        vec![OscType::Int(0), OscType::Int(4800), OscType::Int(2)],
    );
    s.settle_for(4);
    // Silence on the left, a constant on the right, a run of frames at a
    // time: the ring is not a place to put a take.
    for start in (0..4800).step_by(512) {
        let frames = 512.min(4800 - start) as usize;
        let blob: Vec<u8> = (0..frames).flat_map(|_| 0.5f32.to_le_bytes()).collect();
        send(
            &mut s,
            "/buffer_setRangeChannel",
            vec![
                OscType::Int(0),
                OscType::Int(1),
                OscType::Int(start),
                OscType::Blob(blob),
            ],
        );
        s.settle_for(1);
    }
    send(
        &mut s,
        "/graph_new",
        vec![
            OscType::String(mixer::multitrack_name(2)),
            OscType::Int(900),
            OscType::Int(0),
            OscType::Int(0),
        ],
    );
    send(
        &mut s,
        "/graph_addSlot",
        vec![
            OscType::Int(900),
            OscType::String(mixer::TRANSPORT_SLOT.into()),
            OscType::Int(TRACKS),
        ],
    );
    send(
        &mut s,
        "/graph_addSlot",
        vec![
            OscType::Int(TRACKS),
            OscType::String(mixer::track_slot(2)),
            OscType::Int(910),
        ],
    );
    send(
        &mut s,
        "/graph_addSlot",
        vec![
            OscType::Int(910),
            OscType::String(mixer::clip_slot(2)),
            OscType::Int(920),
        ],
    );
    for channel in 0..2 {
        send(
            &mut s,
            "/graph_addSlot",
            vec![
                OscType::Int(920),
                OscType::String(mixer::SOURCE_SLOT.into()),
                OscType::Int(930 + channel),
                OscType::String(mixer::BUF.into()),
                OscType::Float(0.0),
                OscType::String(mixer::CHAN.into()),
                OscType::Float(channel as f32),
                OscType::String(mixer::SPAN.into()),
                OscType::Float(4800.0),
            ],
        );
    }
    send(
        &mut s,
        "/transport_group",
        vec![OscType::Int(0), OscType::Int(TRACKS)],
    );
    s.settle_for(8);
    let refused = fails(&mut s);
    assert!(refused.is_empty(), "nothing was refused: {refused:?}");
    send(&mut s, "/transport_play", vec![OscType::Int(0)]);
    s.settle_for(4);

    let (left, right) = peaks(&mut s, 8);
    assert!(left < 1e-3, "nothing on the left: {left}");
    assert!((right - 0.5).abs() < 0.01, "the take's right side: {right}");
}

/// A def that writes `amp` as a constant onto the bus `out` names: a voice
/// reduced to arithmetic, sounding where an event's `out` key would put it.
fn voice_def(s: &mut NrtSession) {
    let def = serde_json::json!({
        "name": "test.voice",
        "controls": [
            {"name": "out", "default": 0.0},
            {"name": "amp", "default": 0.5},
        ],
        "ugens": [
            {"kind": "Mul", "inputs": [{"control": 1}, {"const": 1.0}]},
            {"kind": "Out", "inputs": [{"control": 0}, {"ugen": 0}]},
        ],
    });
    send(
        s,
        "/def_send",
        vec![
            OscType::String("synth".into()),
            OscType::String(def.to_string()),
        ],
    );
    s.settle_for(4);
}

/// A multitrack with one stereo track holding one **box of notes** over the
/// buses `bus` and `bus + 1`, and a group before the multitrack for its
/// voices. Answers the ids `(multitrack, track, box, voices' group)`.
fn one_box_of_notes(s: &mut NrtSession, bus: i32) -> (i32, i32, i32, i32) {
    send(
        s,
        "/graph_new",
        vec![
            OscType::String(mixer::multitrack_name(2)),
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
            OscType::String(mixer::TRANSPORT_SLOT.into()),
            OscType::Int(TRACKS),
        ],
    );
    send(
        s,
        "/graph_addSlot",
        vec![
            OscType::Int(TRACKS),
            OscType::String(mixer::track_slot(2)),
            OscType::Int(910),
        ],
    );
    send(
        s,
        "/graph_addSlot",
        vec![
            OscType::Int(910),
            OscType::String(mixer::VOICE_SLOT.into()),
            OscType::Int(940),
            OscType::String(mixer::voice_port(0)),
            OscType::Float(bus as f32),
            OscType::String(mixer::voice_port(1)),
            OscType::Float((bus + 1) as f32),
        ],
    );
    // **The voices' group goes before the multitrack**, so a voice writes its
    // bus in the block the box reads it.
    send(
        s,
        "/group_new",
        vec![OscType::Int(800), OscType::Int(2), OscType::Int(900)],
    );
    send(
        s,
        "/transport_group",
        vec![OscType::Int(0), OscType::Int(TRACKS)],
    );
    s.settle_for(8);
    (900, 910, 940, 800)
}

/// **A box of notes is a source of sound inside its track.** Its voices write
/// the box's bus and nothing else, so what reaches the hardware is what the
/// track's strip made of them: the track's gain scales a note and its mute
/// silences it, as they do a take.
#[test]
fn a_box_of_notes_sounds_through_its_tracks_strip() {
    let mut s = session();
    send_defs(&mut s, &[], 2);
    voice_def(&mut s);
    let (_, track, _, voices) = one_box_of_notes(&mut s, 100);
    send(
        &mut s,
        "/synth_new",
        vec![
            OscType::String("test.voice".into()),
            OscType::Int(950),
            OscType::Int(1),
            OscType::Int(voices),
            OscType::String("out".into()),
            OscType::Float(100.0),
            OscType::String("amp".into()),
            OscType::Float(0.5),
        ],
    );
    let refused = fails(&mut s);
    assert!(refused.is_empty(), "nothing was refused: {refused:?}");
    send(&mut s, "/transport_play", vec![OscType::Int(0)]);
    s.settle_for(2);

    let (left, right) = peaks(&mut s, 8);
    assert!(
        (left - 0.5).abs() < 0.01,
        "the voice is heard through the track, at unity: {left}"
    );
    assert!(right < 1e-3, "and only where it wrote: {right}");

    send(
        &mut s,
        "/node_set",
        vec![
            OscType::Int(track),
            OscType::String(mixer::GAIN.into()),
            OscType::Float(0.5),
        ],
    );
    // Past the fader's lag.
    s.settle_for(2);
    let _settling = peaks(&mut s, 40);
    let (left, _) = peaks(&mut s, 8);
    assert!(
        (left - 0.25).abs() < 0.01,
        "the track's gain scales the note: {left}"
    );

    send(
        &mut s,
        "/node_set",
        vec![
            OscType::Int(track),
            OscType::String(mixer::MUTE.into()),
            OscType::Float(1.0),
        ],
    );
    s.settle_for(2);
    let _settling = peaks(&mut s, 40);
    let (left, right) = peaks(&mut s, 8);
    assert!(
        left < 1e-3 && right < 1e-3,
        "and its mute silences it: {left}, {right}"
    );
}

/// **The built-in `default` plays where `out` says.** Sent to a box's bus it
/// reaches the hardware through the track alone, on both sides -- it pans --
/// and a muted track leaves nothing of it.
#[test]
fn the_default_def_sounds_into_the_bus_out_names() {
    let mut s = session();
    send_defs(&mut s, &[], 2);
    let (_, track, _, voices) = one_box_of_notes(&mut s, 100);
    send(
        &mut s,
        "/synth_new",
        vec![
            OscType::String("default".into()),
            OscType::Int(950),
            OscType::Int(1),
            OscType::Int(voices),
            OscType::String("out".into()),
            OscType::Float(100.0),
        ],
    );
    send(&mut s, "/transport_play", vec![OscType::Int(0)]);
    s.settle_for(2);
    let (left, right) = peaks(&mut s, 16);
    assert!(
        left > 0.01 && right > 0.01,
        "heard on both sides: {left}, {right}"
    );

    send(
        &mut s,
        "/node_set",
        vec![
            OscType::Int(track),
            OscType::String(mixer::MUTE.into()),
            OscType::Float(1.0),
        ],
    );
    s.settle_for(2);
    let _settling = peaks(&mut s, 40);
    let (left, right) = peaks(&mut s, 16);
    assert!(
        left < 1e-3 && right < 1e-3,
        "nothing of it past a muted track: {left}, {right}"
    );
}
