//! A gain automation as existing server gain operations, without samples.

use crate::{apply::Step, points};
use clausters_core::osc::{OscMessage, OscType};
use clausters_core::tempoclock::secs_to_samples;
use clausters_document::{Point, timebase::Second};

/// Render a gain curve over every channel of an owned buffer. Values before
/// the first point and after the last are held; segment shapes are unchanged.
/// Work and payload scale with points, never with the number of samples.
pub fn steps(buffer: i32, frames: u64, rate: f64, curve: &[Point<Second>]) -> Vec<Step> {
    let mut out = Vec::new();
    let Some(first) = curve.first() else {
        return out;
    };
    let frame = |t: f64| (secs_to_samples(t, rate).max(0) as u64).min(frames);
    let mut gain = |start: u64, end: u64, from: f64, to: f64, shape: f64, curvature: f64| {
        if end <= start {
            return;
        }
        out.push(Step::Send(OscMessage {
            addr: "/buffer_gain".into(),
            args: vec![
                OscType::Int(buffer),
                OscType::Int(start as i32),
                OscType::Int((end - start) as i32),
                OscType::Float(from as f32),
                OscType::Float(to as f32),
                OscType::Int(shape as i32),
                OscType::Float(curvature as f32),
            ],
        }));
        out.push(Step::AwaitDone {
            command: "/buffer_gain".into(),
            index: Some(buffer),
        });
    };
    gain(0, frame(first.at.0), first.value, first.value, 1.0, 0.0);
    for pair in curve.windows(2) {
        let q = points::quad(pair[0].at.0, &pair[0]);
        gain(
            frame(pair[0].at.0),
            frame(pair[1].at.0),
            pair[0].value,
            pair[1].value,
            q[2],
            q[3],
        );
    }
    let last = curve.last().unwrap();
    gain(frame(last.at.0), frames, last.value, last.value, 1.0, 0.0);
    out
}
