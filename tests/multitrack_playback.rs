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
    multitrack.tracks[0].lanes[0].place(Region::new(
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
