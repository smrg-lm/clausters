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
use clausters_editing::audio_playback::{AUDIO_EDITOR_TRANSPORT, AudioEditorPlayback, Pass};

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
    let mut playback = AudioEditorPlayback::new(Endpoint::default(), AUDIO_EDITOR_TRANSPORT);
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
    let mut playback = AudioEditorPlayback::new(Endpoint::default(), AUDIO_EDITOR_TRANSPORT);
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
