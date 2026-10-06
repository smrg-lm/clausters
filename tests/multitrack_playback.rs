//! **The multitrack's playback, heard rather than inspected**: the steps
//! `clausters_editing::playback::MultitrackPlayback` answers, carried out on an
//! offline server.

#![cfg(feature = "synth")]

use std::collections::HashMap;

use clausters::rosc::OscType;
use clausters::server::nrtsession::{NrtSession, SessionConfig};
use clausters_core::ids::{IdShare, IdSpaces, ServerShape};
use clausters_document::multitrack::nodes::SourceInfo;
use clausters_document::multitrack::{Content, Multitrack, Region, Track};
use clausters_document::{
    Lifetime, NodeId, Second, SegmentRef, SegmentSource, SourceId, SourceRef,
};
use clausters_editing::apply::{Endpoint, Step};
use clausters_editing::playback::MultitrackPlayback;

const SR: f64 = 48_000.0;
const BLOCK: usize = 64;

fn send(s: &mut NrtSession, addr: &str, args: Vec<OscType>) {
    assert!(s.send_msg(addr, args).expect("encode"), "ring full");
}

/// Carries the steps out: every message sent, every wait a settle.
fn run(s: &mut NrtSession, steps: Vec<Step>) {
    for step in steps {
        match step {
            Step::Send(m) => send(s, &m.addr, m.args),
            Step::AwaitDone { .. } | Step::Sync(_) => {
                s.settle_for(4);
            }
        }
    }
    s.settle_for(4);
}

/// **A box longer than its source is heard for as long as the source, and
/// the rest is exactly zero** -- not the source's last frame held on the
/// track until the box closes. A box of sixteen blocks over a take of four.
#[test]
fn a_box_longer_than_its_source_is_silent_past_its_end() {
    let mut s = NrtSession::open(&SessionConfig {
        sample_rate: SR,
        channels: 2,
        ..Default::default()
    })
    .expect("open");
    let take = 4 * BLOCK;
    send(
        &mut s,
        "/buffer_alloc",
        vec![OscType::Int(0), OscType::Int(take as i32), OscType::Int(1)],
    );
    s.settle_for(4);
    send(
        &mut s,
        "/buffer_fill",
        vec![
            OscType::Int(0),
            OscType::Int(0),
            OscType::Int(take as i32),
            OscType::Float(0.5),
        ],
    );
    s.settle_for(4);

    let long = (16 * BLOCK) as f64 / SR;
    let mut multitrack = Multitrack {
        tracks: vec![Track::new(NodeId(10), NodeId(11))],
        ..Multitrack::default()
    };
    multitrack.tracks[0].take_lanes[0].place(Region::new(
        NodeId(20),
        Second(0.0),
        Second(long),
        Content::window(SegmentRef {
            source: SegmentSource::Samples(SourceRef {
                source: SourceId(1),
                lifetime: Lifetime::Session,
                generation: 0,
                range: None,
            }),
            start: 0.0,
            duration: long,
        }),
    ));
    let sources = HashMap::from([(
        SourceId(1),
        SourceInfo {
            buffer: 0,
            channels: 1,
            duration: Some(take as f64 / SR),
        },
    )]);
    let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
    let mut playback = MultitrackPlayback::new(Endpoint::default());
    let steps = playback
        .sync(&multitrack, SR, &sources, 1.0, &mut ids)
        .unwrap();
    run(&mut s, steps);
    let steps = playback.play();
    run(&mut s, steps);

    let out = s.run_to_vec((24 * BLOCK) as u64).expect("the render ran");
    let left: Vec<f32> = out.as_chunks::<2>().0.iter().map(|f| f[0]).collect();
    let heard = left
        .iter()
        .position(|x| x.abs() > 0.1)
        .expect("the take is heard");
    let last = left
        .iter()
        .rposition(|x| *x != 0.0)
        .expect("something sounded");
    assert!(
        last < heard + take,
        "heard for the take's {take} frames and no longer: sound until {last}, from {heard}"
    );
    assert!(
        left[heard + take..].iter().all(|x| *x == 0.0),
        "and past it, exactly zero"
    );
}

/// **A track's gain automation reaches the notes of a box on it.** A box of
/// notes is a source of sound inside its track, so a curve over the track's
/// gain shapes a note the way it shapes a take: closed, the note is not
/// heard; open, it is, at the level its own `amp` says.
///
/// The note is a constant on the bus its `out` names, so what reaches the
/// hardware is the track's strip and nothing else.
#[test]
fn a_tracks_gain_curve_reaches_the_notes_of_its_boxes() {
    under_a_tracks_gain_curve(Vec::new());
}

/// **And a note that curves shape.** A note with a curve of its own plays in
/// a graph, beside the readers of its curves, and that graph sounds into the
/// box as a plain synth does: the track's curve is after the note's own.
#[test]
fn a_tracks_gain_curve_reaches_a_note_its_own_curves_shape() {
    use clausters_document::{Opaque, Point};
    use clausters_editing::notes_playback::PlacedCurve;

    let level = |at: f64| Point {
        at,
        value: 0.5,
        data: Opaque::none(),
    };
    under_a_tracks_gain_curve(vec![PlacedCurve {
        id: "c".into(),
        scope: String::new(),
        target: serde_json::json!({"control": "amp"}),
        points: vec![level(0.0), level(1.0)],
    }]);
}

/// One note of half amplitude over a box of a second, with `curves` of its
/// own, on a track whose gain is closed and then open: heard only once it is
/// open, at its own level.
fn under_a_tracks_gain_curve(curves: Vec<clausters_editing::notes_playback::PlacedCurve>) {
    use clausters_document::multitrack::Automation;
    use clausters_document::{Opaque, Point};
    use clausters_editing::notes_playback::{Placed, Placement};

    let mut s = NrtSession::open(&SessionConfig {
        sample_rate: SR,
        channels: 2,
        ..Default::default()
    })
    .expect("open");
    let voice = serde_json::json!({
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
        &mut s,
        "/def_send",
        vec![
            OscType::String("synth".into()),
            OscType::String(voice.to_string()),
        ],
    );
    s.settle_for(4);

    // A box of a second, whose source is a sequence: no buffer is behind it.
    let opens = (64 * BLOCK) as f64 / SR;
    let mut multitrack = Multitrack {
        tracks: vec![Track::new(NodeId(10), NodeId(11))],
        ..Multitrack::default()
    };
    multitrack.tracks[0].take_lanes[0].place(Region::new(
        NodeId(20),
        Second(0.0),
        Second(1.0),
        Content::window(SegmentRef {
            source: SegmentSource::Samples(SourceRef {
                source: SourceId(1),
                lifetime: Lifetime::Session,
                generation: 0,
                range: None,
            }),
            start: 0.0,
            duration: 1.0,
        }),
    ));
    // The track's gain: closed, then open from `opens` on.
    let point = |at: f64, value: f64| Point {
        at,
        value,
        data: Opaque::none(),
    };
    let mut gain = Automation::new(NodeId(30), Opaque(serde_json::json!({"port": "gain"})));
    gain.points = vec![
        point(0.0, 0.0),
        point(opens, 0.0),
        point(opens + BLOCK as f64 / SR, 1.0),
    ];
    multitrack.tracks[0].automation = vec![gain];

    let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
    let mut playback = MultitrackPlayback::new(Endpoint::default());
    let steps = playback
        .sync(&multitrack, SR, &HashMap::new(), 1.0, &mut ids)
        .unwrap();
    run(&mut s, steps);
    // One note over the whole box, of the box (its scope is the region).
    let placed = Placement {
        events: vec![Placed {
            start: 0.0,
            end: 1.0,
            keys: serde_json::from_value(serde_json::json!({
                "instrument": "test.voice", "amp": 0.5,
            }))
            .unwrap(),
            id: "1".into(),
            scope: "20".into(),
            curves,
        }],
        curves: Vec::new(),
    };
    let steps = playback.notes(&placed, &mut ids).unwrap();
    run(&mut s, steps);
    let steps = playback.play();
    run(&mut s, steps);

    // In stretches, a serving turn between them: the lane is fed ahead of
    // the position by the turn, as a take is not.
    let mut left = Vec::new();
    for _ in 0..12 {
        let out = s.run_to_vec((16 * BLOCK) as u64).expect("the render ran");
        left.extend(out.as_chunks::<2>().0.iter().map(|f| f[0]));
        s.settle_for(1);
    }
    let closed = &left[8 * BLOCK..56 * BLOCK];
    let open = &left[96 * BLOCK..160 * BLOCK];
    let peak = |frames: &[f32]| frames.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(
        peak(closed) < 1e-3,
        "under a closed track the note is not heard: {}",
        peak(closed)
    );
    assert!(
        (peak(open) - 0.5).abs() < 0.02,
        "and under an open one it is, at its own level: {}",
        peak(open)
    );
}
