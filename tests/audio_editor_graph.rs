//! **The audio editor's node system, heard rather than inspected.**
//!
//! `clausters_core::audio_editor` says what the editor is as nodes, and
//! `clausters_editing::audio_playback` what steps make them; these carry the
//! steps out on an offline server and measure what came out: the take is
//! heard, past its end the output is exactly zero, and neither a play nor a
//! stop is a step -- the transport's ramp and the output's declick see to
//! that -- while the meter reads the level and falls on a pause.

#![cfg(feature = "synth")]

use clausters::rosc::OscType;
use clausters::server::nrtsession::{NrtSession, SessionConfig};
use clausters_core::ids::{IdShare, IdSpaces, ServerShape};
use clausters_editing::apply::{Endpoint, Step};
use clausters_editing::audio_playback::{AudioEditorPlayback, Pass};

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

/// Every `/fail` that has arrived, as text.
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

/// A mono take of `frames` frames holding a constant: what the chain did to it
/// is arithmetic rather than a spectrum, and a hard edge anywhere is a step of
/// the whole level.
fn take(s: &mut NrtSession, frames: usize, value: f32) {
    send(
        s,
        "/buffer_alloc",
        vec![
            OscType::Int(0),
            OscType::Int(frames as i32),
            OscType::Int(1),
        ],
    );
    s.settle_for(4);
    send(
        s,
        "/buffer_fill",
        vec![
            OscType::Int(0),
            OscType::Int(0),
            OscType::Int(frames as i32),
            OscType::Float(value),
        ],
    );
    s.settle_for(4);
}

/// Renders `blocks` blocks and answers the left and right channels.
fn render(s: &mut NrtSession, blocks: usize) -> (Vec<f32>, Vec<f32>) {
    let out = s
        .run_to_vec((blocks * BLOCK) as u64)
        .expect("the render ran");
    out.as_chunks::<2>().0.iter().map(|f| (f[0], f[1])).unzip()
}

/// The largest step between two consecutive samples: a click is a step of
/// the whole level, a ramp of five milliseconds a step of a two-hundredth.
fn largest_step(signal: &[f32]) -> f32 {
    signal
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
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

/// **A pass is heard on both sides, ramps in, ends where the take does, and
/// is silent after**: a mono take plays on the left and the right, the play
/// rises over the ramp rather than stepping, the end mark's ramp brings the
/// level to zero on the take's last frame, and past it the output is exactly
/// zero -- not the take's last sample held.
#[test]
fn a_pass_is_declicked_and_ends_where_the_take_does() {
    let mut s = session();
    let frames = 40 * BLOCK;
    take(&mut s, frames, 0.5);
    let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
    let mut playback = AudioEditorPlayback::new(Endpoint::default());
    let steps = playback
        .sync(1, 0, 1, frames as u64, SR, SR, &mut ids)
        .unwrap();
    run(&mut s, steps);
    let steps = playback.play(
        1,
        0,
        Pass::Until {
            end: frames as u64,
            back: 0,
        },
    );
    run(&mut s, steps);
    assert_eq!(fails(&mut s), Vec::<String>::new());

    let (left, right) = render(&mut s, 60);
    let heard = left
        .iter()
        .position(|x| *x > 0.0)
        .expect("the take is heard");
    assert!(
        left[heard + 300] > 0.49 && right[heard + 300] > 0.49,
        "on both sides, at unity: {} {}",
        left[heard + 300],
        right[heard + 300]
    );
    assert!(
        largest_step(&left) < 0.01 && largest_step(&right) < 0.01,
        "neither the play nor the end is a step: {}",
        largest_step(&left)
    );
    let over = heard + frames;
    assert!(
        left[over..].iter().chain(&right[over..]).all(|x| *x == 0.0),
        "and past the take's end the output is exactly zero"
    );
}

/// **A stop mid-pass is a ramp, and the meter falls**: the output goes to zero
/// with no step, the meter reads the take's level while it plays and falls
/// once it is paused, and closing frees what the editor made.
#[test]
fn a_stop_is_declicked_and_the_meter_falls() {
    let mut s = session();
    let frames = 400 * BLOCK;
    take(&mut s, frames, 0.5);
    let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
    let mut playback = AudioEditorPlayback::new(Endpoint::default());
    let steps = playback
        .sync(1, 0, 1, frames as u64, SR, SR, &mut ids)
        .unwrap();
    run(&mut s, steps);
    let steps = playback.play(
        1,
        0,
        Pass::Loop {
            from: 0,
            to: frames as u64,
        },
    );
    run(&mut s, steps);
    let (playing, _) = render(&mut s, 40);
    assert!(playing.iter().any(|x| *x > 0.49), "the take plays");
    let (bus, channels) = playback.meters().expect("a meter");
    assert_eq!(channels, 2);
    let level = bus_value(&mut s, bus);
    assert!(level > 0.4, "the meter reads the take: {level}");

    let steps = playback.pause();
    run(&mut s, steps);
    let (stopping, _) = render(&mut s, 40);
    assert!(
        largest_step(
            &playing[playing.len() - 1..]
                .iter()
                .chain(&stopping)
                .copied()
                .collect::<Vec<_>>()
        ) < 0.01,
        "the stop is a ramp, not a step"
    );
    assert!(
        stopping[stopping.len() - BLOCK..].iter().all(|x| *x == 0.0),
        "and then silence"
    );
    // The meter falls at its own ballistics (20 dB a second), since the
    // output is not frozen: two seconds are 40 dB.
    let (_, _) = render(&mut s, 1_600);
    let fallen = bus_value(&mut s, bus);
    assert!(
        fallen < 0.02 * level,
        "the meter falls on a pause: {fallen}"
    );

    let steps = playback.close(&mut ids).unwrap();
    run(&mut s, steps);
    assert_eq!(fails(&mut s), Vec::<String>::new());
    assert_eq!(playback.node_count(), 0);
}

/// The gain insert follows the transport and uses a linear bypass with an
/// exact dry endpoint. Repeated resume addresses the synth paused by EnvGen.
#[test]
fn gain_preview_bypasses_without_a_step_and_resumes() {
    use clausters_document::timebase::Second;
    use clausters_editing::points::point;
    let mut s = session();
    take(&mut s, 48000, 0.5);
    let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
    let mut playback = AudioEditorPlayback::new(Endpoint::default());
    run(
        &mut s,
        playback.sync(1, 0, 1, 48000, SR, SR, &mut ids).unwrap(),
    );
    ids.alloc(clausters_core::ids::Space::Buffers, 1).unwrap();
    let points = vec![
        point(Second(0.0), 0.25, 1.0, 0.0),
        point(Second(1.0), 0.25, 1.0, 0.0),
    ];
    run(
        &mut s,
        playback.gain(1, points.clone(), true, &mut ids).unwrap(),
    );
    run(
        &mut s,
        playback.play(1, 0, Pass::Loop { from: 0, to: 48000 }),
    );
    let (wet, _) = render(&mut s, 20);
    assert!(
        (wet[wet.len() - 1] - 0.125).abs() < 1e-5,
        "last {} fails {:?}",
        wet[wet.len() - 1],
        fails(&mut s)
    );
    for on in [false, true, false, true] {
        run(
            &mut s,
            playback.gain(1, points.clone(), on, &mut ids).unwrap(),
        );
        let (signal, _) = render(&mut s, 20);
        assert!(signal.iter().all(|x| x.is_finite()));
        assert!(
            largest_step(&signal) < 0.003,
            "bypass step {}",
            largest_step(&signal)
        );
        let want = if on { 0.125 } else { 0.5 };
        assert!(
            (signal[signal.len() - 1] - want).abs() < 1e-5,
            "on {on}: {}",
            signal[signal.len() - 1]
        );
    }
    run(&mut s, playback.close(&mut ids).unwrap());
    assert!(fails(&mut s).is_empty());
}

/// Every segment shape is shared with Apply, including held ends. This
/// compares complete node systems, not two copies of envelope arithmetic.
#[test]
fn preview_matches_apply_for_every_gain_shape() {
    use clausters_document::timebase::Second;
    use clausters_editing::points::point;
    for shape in 0..=8 {
        let curve = vec![
            point(Second(0.005), 0.2, shape as f64, -3.0),
            point(Second(0.025), 1.6, 1.0, 0.0),
        ];
        let mut outputs = Vec::new();
        for preview in [true, false] {
            let mut s = session();
            take(&mut s, 4096, 0.5);
            let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
            ids.alloc(clausters_core::ids::Space::Buffers, 1).unwrap();
            let mut playback = AudioEditorPlayback::new(Endpoint::default());
            if !preview {
                run(&mut s, clausters_editing::gain::steps(0, 4096, SR, &curve));
            }
            run(
                &mut s,
                playback.sync(1, 0, 1, 4096, SR, SR, &mut ids).unwrap(),
            );
            if preview {
                run(
                    &mut s,
                    playback.gain(1, curve.clone(), true, &mut ids).unwrap(),
                );
            }
            run(
                &mut s,
                playback.play(1, 0, Pass::Until { end: 4096, back: 0 }),
            );
            let (left, right) = render(&mut s, 70);
            assert!(left.iter().chain(&right).all(|x| x.is_finite()));
            assert_eq!(left, right);
            assert!(fails(&mut s).is_empty());
            outputs.push(left);
        }
        let error = outputs[0]
            .iter()
            .zip(&outputs[1])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        let at = outputs[0]
            .iter()
            .zip(&outputs[1])
            .position(|(a, b)| (a - b).abs() == error)
            .unwrap();
        assert!(
            error < 2e-5,
            "shape {shape}: error {error} at {at}: {} {}",
            outputs[0][at],
            outputs[1][at]
        );
    }
}

/// **An edit with the insert already sounding is heard as Apply would be**:
/// the curve moved under a preview replaces the insert's table, and the
/// output matches the same curve applied to the take, with no step where
/// the first point after zero lands.
#[test]
fn a_curve_edited_under_preview_matches_apply() {
    use clausters_document::timebase::Second;
    use clausters_editing::points::point;
    let before = vec![
        point(Second(0.0), 1.0, 1.0, 0.0),
        point(Second(0.08), 1.0, 1.0, 0.0),
    ];
    let after = vec![
        point(Second(0.0), 1.0, 1.0, 0.0),
        point(Second(0.02), 0.2, 1.0, 0.0),
        point(Second(0.06), 1.0, 1.0, 0.0),
    ];
    let mut outputs = Vec::new();
    for preview in [true, false] {
        let mut s = session();
        take(&mut s, 4096, 0.5);
        let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
        ids.alloc(clausters_core::ids::Space::Buffers, 1).unwrap();
        let mut playback = AudioEditorPlayback::new(Endpoint::default());
        if !preview {
            run(&mut s, clausters_editing::gain::steps(0, 4096, SR, &after));
        }
        run(
            &mut s,
            playback.sync(1, 0, 1, 4096, SR, SR, &mut ids).unwrap(),
        );
        if preview {
            run(
                &mut s,
                playback.gain(1, before.clone(), true, &mut ids).unwrap(),
            );
            run(
                &mut s,
                playback.gain(1, after.clone(), true, &mut ids).unwrap(),
            );
        }
        run(
            &mut s,
            playback.play(1, 0, Pass::Until { end: 4096, back: 0 }),
        );
        let (left, _) = render(&mut s, 70);
        assert!(fails(&mut s).is_empty());
        outputs.push(left);
    }
    let error = outputs[0]
        .iter()
        .zip(&outputs[1])
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);
    let at = outputs[0]
        .iter()
        .zip(&outputs[1])
        .position(|(a, b)| (a - b).abs() == error)
        .unwrap();
    assert!(
        error < 2e-5,
        "error {error} at {at}: {} {}; steps {} {}",
        outputs[0][at],
        outputs[1][at],
        largest_step(&outputs[0]),
        largest_step(&outputs[1])
    );
}
