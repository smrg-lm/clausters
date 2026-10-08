//! **The multitrack's playback, heard rather than inspected**: the steps
//! `clausters_editing::playback::MultitrackPlayback` answers, carried out on an
//! offline server.

#![cfg(feature = "synth")]

use std::collections::HashMap;

use clausters::rosc::OscType;
use clausters::server::nrtsession::{NrtSession, SessionConfig};
use clausters_core::ids::{IdShare, IdSpaces, ServerShape};
use clausters_document::multitrack::Window;
use clausters_document::multitrack::nodes::SourceInfo;
use clausters_document::multitrack::{Content, Fade, Multitrack, Region, Track};
use clausters_document::{Lifetime, NodeId, Second, SourceId, SourceRef};
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
    let mut region = Region::new(
        NodeId(20),
        Second(0.0),
        Second(long),
        Content::window(Window {
            source: SourceRef {
                source: SourceId(1),
                lifetime: Lifetime::Session,
                generation: 0,
                range: None,
            },
            start: 0.0,
            duration: long,
        }),
    );
    // Butt edges: the take is shorter than the multitrack's default fade, and
    // what is measured here is where it stops, not how.
    region.fade_in = Some(Fade::of(Second(0.0)));
    region.fade_out = Some(Fade::of(Second(0.0)));
    multitrack.tracks[0].take_lanes[0].place(region);
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

/// **A mono track in a stereo multitrack sounds, and widening it while it
/// sounds makes it again.** The document says how wide a track is; the plan
/// names the slot that width fills, and a track whose width moved is another
/// graph in another slot -- freed and made again with its clip, through the
/// same `sync` every other edit takes.
#[test]
fn a_track_s_width_is_the_document_s_and_may_change_while_it_plays() {
    let mut s = NrtSession::open(&SessionConfig {
        sample_rate: SR,
        channels: 2,
        ..Default::default()
    })
    .expect("open");
    let take = 4096 * BLOCK;
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
            OscType::Float(1.0),
        ],
    );
    s.settle_for(4);

    let long = take as f64 / SR;
    let mut track = Track::new(NodeId(10), NodeId(11));
    track.channels = 1;
    let mut multitrack = Multitrack {
        channels: 2,
        tracks: vec![track],
        ..Multitrack::default()
    };
    multitrack.tracks[0].take_lanes[0].place(Region::new(
        NodeId(20),
        Second(0.0),
        Second(long),
        Content::window(Window {
            source: SourceRef {
                source: SourceId(1),
                lifetime: Lifetime::Session,
                generation: 0,
                range: None,
            },
            start: 0.0,
            duration: long,
        }),
    ));
    let sources = HashMap::from([(
        SourceId(1),
        SourceInfo {
            buffer: 0,
            channels: 1,
            duration: Some(long),
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

    // The last block of a stretch, past every fader's lag.
    let sides = |s: &mut NrtSession| {
        let out = s.run_to_vec((48 * BLOCK) as u64).expect("the render ran");
        let frame = out.as_chunks::<2>().0.last().copied().expect("a frame");
        (frame[0], frame[1])
    };
    let centre = 1.0 / 2.0f32.sqrt();
    let (left, right) = sides(&mut s);
    assert!(
        (left - centre).abs() < 0.02 && (right - centre).abs() < 0.02,
        "a mono track is panned into the master, -3 dB a side: {left}, {right}"
    );

    // The same track, stereo now: the mono take is panned into *it*, and its
    // balance leaves the centre alone.
    multitrack.tracks[0].channels = 2;
    let steps = playback
        .sync(&multitrack, SR, &sources, 1.0, &mut ids)
        .unwrap();
    assert!(!steps.is_empty(), "a width is not something a set says");
    run(&mut s, steps);
    let (left, right) = sides(&mut s);
    assert!(
        (left - centre).abs() < 0.02 && (right - centre).abs() < 0.02,
        "made again and sounding as before: {left}, {right}"
    );
    // And settled: the same document again asks for nothing.
    let again = playback
        .sync(&multitrack, SR, &sources, 1.0, &mut ids)
        .unwrap();
    assert!(
        again.is_empty(),
        "nothing left to do: {} steps",
        again.len()
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

    let opens = (64 * BLOCK) as f64 / SR;
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
    let left = a_note_in_a_box(curves, |multitrack| {
        multitrack.tracks[0].automation = vec![gain];
    });
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

/// **A box of notes fades at its own edges**, as a box of samples does: a
/// constant note over a box of a fifth of a second with a straight fade of
/// 50 ms at each end is at half its level half way into either, and silent
/// past the box's end.
#[test]
fn a_box_of_notes_fades_at_its_own_edges() {
    let linear = clausters_core::envshape::SHAPE_LINEAR;
    let left = a_note_in_a_box(Vec::new(), |multitrack| {
        let region = &mut multitrack.tracks[0].take_lanes[0].regions[0];
        region.length = Second(0.2);
        region.fade_in = Some(Fade::of(Second(0.05)).shaped(linear, 0.0));
        region.fade_out = Some(Fade::of(Second(0.05)).shaped(linear, 0.0));
    });
    let at = |secs: f64| left[(secs * SR) as usize];
    let near = |got: f32, want: f32, what: &str| {
        assert!((got - want).abs() < 0.01, "{what}: {got}, not {want}");
    };
    near(at(0.025), 0.25, "half way into the fade in");
    near(at(0.1), 0.5, "between the fades, the note's own level");
    near(at(0.175), 0.25, "half way into the fade out");
    near(at(0.21), 0.0, "and past the box, nothing");
}

/// One note of half amplitude, a constant, over a box of a second that is
/// the only one on its track, with `curves` of its own and the multitrack as
/// `shape` leaves it: the left channel of what is heard, over its first
/// quarter second.
fn a_note_in_a_box(
    curves: Vec<clausters_editing::notes_playback::PlacedCurve>,
    shape: impl FnOnce(&mut Multitrack),
) -> Vec<f32> {
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
    let mut multitrack = Multitrack {
        tracks: vec![Track::new(NodeId(10), NodeId(11))],
        ..Multitrack::default()
    };
    multitrack.tracks[0].take_lanes[0].place(Region::new(
        NodeId(20),
        Second(0.0),
        Second(1.0),
        Content::window(Window {
            source: SourceRef {
                source: SourceId(1),
                lifetime: Lifetime::Session,
                generation: 0,
                range: None,
            },
            start: 0.0,
            duration: 1.0,
        }),
    ));
    shape(&mut multitrack);

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
    left
}

/// **A region's own fades are heard at its edges**, each an envelope segment in
/// its own shape: from silence on its first frame to full level `fade_in` in,
/// and back to silence on its last. A constant take, so the ramp is the only thing that moves; the
/// box sits well after the play's own declick, so that ramp is not the one
/// measured.
#[test]
fn a_region_fades_in_and_out_at_its_own_edges() {
    let mut s = NrtSession::open(&SessionConfig {
        sample_rate: SR,
        channels: 2,
        ..Default::default()
    })
    .expect("open");
    let take = 32 * BLOCK;
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

    let fade = 4 * BLOCK;
    let secs = |frames: usize| frames as f64 / SR;
    let at = 32 * BLOCK;
    let mut multitrack = Multitrack {
        tracks: vec![Track::new(NodeId(10), NodeId(11))],
        ..Multitrack::default()
    };
    let mut region = Region::new(
        NodeId(20),
        Second(secs(at)),
        Second(secs(take)),
        Content::window(Window {
            source: SourceRef {
                source: SourceId(1),
                lifetime: Lifetime::Session,
                generation: 0,
                range: None,
            },
            start: 0.0,
            duration: secs(take),
        }),
    );
    // In: equal power, the default shape. Out: a straight line, chosen.
    region.fade_in = Some(Fade::of(Second(secs(fade))));
    region.fade_out =
        Some(Fade::of(Second(secs(fade))).shaped(clausters_core::envshape::SHAPE_LINEAR, 0.0));
    multitrack.tracks[0].take_lanes[0].place(region);
    let sources = HashMap::from([(
        SourceId(1),
        SourceInfo {
            buffer: 0,
            channels: 1,
            duration: Some(secs(take)),
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

    let out = s
        .run_to_vec((at + take + 16 * BLOCK) as u64)
        .expect("the render ran");
    let left: Vec<f32> = out.as_chunks::<2>().0.iter().map(|f| f[0]).collect();
    let first = left
        .iter()
        .position(|x| *x != 0.0)
        .expect("the take is heard");
    let last = left.iter().rposition(|x| *x != 0.0).expect("it sounded");
    let full = left[first + take / 2];
    assert!(full > 0.1, "the middle is the take at full level: {full}");
    let near = |got: f32, want: f32, what: &str| {
        assert!(
            (got - want).abs() < 0.01 * full,
            "{what}: {got}, not {want}"
        );
    };
    // Rising, equal power: a quarter sine through the fade, its first frame
    // one step of it and half way at sin(pi/4).
    let quarter = |t: f32| (std::f32::consts::FRAC_PI_2 * t).sin();
    near(
        left[first],
        full * quarter(1.0 / fade as f32),
        "the first frame",
    );
    near(
        left[first + fade / 2 - 1],
        full * quarter(0.5),
        "half way in",
    );
    near(left[first + fade + 4], full, "past the fade in");
    // Falling in a straight line, onto the last frame of the box.
    assert_eq!(last + 1 - first, take, "the box sounds for its length");
    near(left[last - fade / 2], full * 0.5, "half way out");
    near(left[last - fade - 4], full, "before the fade out");
}

/// **Two regions that overlap crossfade at equal power**: two constant takes,
/// the second placed over the last quarter of the first. Over the overlap the
/// first fades out and the second fades in, each over all of it, so half way
/// through each is at sin(pi/4) of its level -- and two uncorrelated sources
/// there would keep their summed power. Before and after, each plays alone at
/// full level.
#[test]
fn an_overlap_crossfades_at_equal_power() {
    let mut s = NrtSession::open(&SessionConfig {
        sample_rate: SR,
        channels: 2,
        ..Default::default()
    })
    .expect("open");
    let take = 32 * BLOCK;
    for buffer in 0..2 {
        send(
            &mut s,
            "/buffer_alloc",
            vec![
                OscType::Int(buffer),
                OscType::Int(take as i32),
                OscType::Int(1),
            ],
        );
        s.settle_for(4);
        send(
            &mut s,
            "/buffer_fill",
            vec![
                OscType::Int(buffer),
                OscType::Int(0),
                OscType::Int(take as i32),
                OscType::Float(0.5),
            ],
        );
        s.settle_for(4);
    }
    let secs = |frames: usize| frames as f64 / SR;
    let window = |source: u64| {
        Content::window(Window {
            source: SourceRef {
                source: SourceId(source),
                lifetime: Lifetime::Session,
                generation: 0,
                range: None,
            },
            start: 0.0,
            duration: secs(take),
        })
    };
    let at = 32 * BLOCK;
    let overlap = 8 * BLOCK;
    let mut multitrack = Multitrack {
        tracks: vec![Track::new(NodeId(10), NodeId(11))],
        ..Multitrack::default()
    };
    // No default fade, so the only fades are the crossfade's.
    multitrack.defaults.fade = None;
    let lane = &mut multitrack.tracks[0].take_lanes[0];
    lane.place(Region::new(
        NodeId(20),
        Second(secs(at)),
        Second(secs(take)),
        window(1),
    ));
    lane.place(Region::new(
        NodeId(21),
        Second(secs(at + take - overlap)),
        Second(secs(take)),
        window(2),
    ));
    let sources = HashMap::from([
        (
            SourceId(1),
            SourceInfo {
                buffer: 0,
                channels: 1,
                duration: Some(secs(take)),
            },
        ),
        (
            SourceId(2),
            SourceInfo {
                buffer: 1,
                channels: 1,
                duration: Some(secs(take)),
            },
        ),
    ]);
    let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
    let mut playback = MultitrackPlayback::new(Endpoint::default());
    let steps = playback
        .sync(&multitrack, SR, &sources, 1.0, &mut ids)
        .unwrap();
    run(&mut s, steps);
    let steps = playback.play();
    run(&mut s, steps);

    let out = s
        .run_to_vec((at + 2 * take + 16 * BLOCK) as u64)
        .expect("the render ran");
    let left: Vec<f32> = out.as_chunks::<2>().0.iter().map(|f| f[0]).collect();
    let first = left.iter().position(|x| *x != 0.0).expect("it sounded");
    let alone = left[first + take / 4];
    assert!(alone > 0.1, "the first take alone: {alone}");
    // Half way through the overlap both are at sin(pi/4) of the level one
    // plays at alone, and these two are the same constant, so they sum to
    // twice that.
    let middle = left[first + take - overlap / 2];
    let quarter = std::f32::consts::FRAC_PI_4.sin();
    assert!(
        (middle - 2.0 * quarter * alone).abs() < 0.02 * alone,
        "half way through the crossfade: {middle}, not {}",
        2.0 * quarter * alone
    );
    // After it, the second alone at full level.
    let after = left[first + take + take / 4];
    assert!(
        (after - alone).abs() < 0.01 * alone,
        "the second take alone: {after}"
    );
}
