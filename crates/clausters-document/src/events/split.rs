//! **A note cut in two at a beat**, as a region is split in the multitrack:
//! the first half keeps the event and its id, and the second is a new event
//! with the first's keys, from the cut to where the note ended.
//!
//! - **Its length.** A note written as notation (it carries a `value`) is cut
//!   on its written length, `dur`, and each half's `value` is written again
//!   from its own -- taken away where no written value is that long, so the
//!   page reads the one played; a `sustain` it states is shared out in
//!   proportion. Any other note is cut on what it sounds: each half sounds to
//!   its end, and its `dur` follows.
//! - **Its curves.** Each curve of the note is cut at the point: the first
//!   half keeps the points before it, the second the points after it, and both
//!   take the value the curve had there, so neither half jumps. The second
//!   half's curves are new curves, with ids of their own.
//! - **Its joins.** The second half is a new attack, so it goes on from
//!   nothing; a note that went on from the cut note goes on from the second
//!   half, and a spanner that ended on the note ends on it too.

use serde_json::{Value, json};

use clausters_core::event::render::{self, Type};

use super::EventSequence;
use super::reading::{Transcription, written_value};
use crate::multitrack::nodes::value_at;
use crate::timebase::Beat;
use crate::{NodeId, Opaque, Point};

/// Within this of a note's start or end, a beat cuts nothing.
const EPSILON: f64 = 1e-9;

/// **Cuts every note of `ids` that `at` falls inside** (see the module), and
/// answers the ids of the second halves, in the order of `ids`.
///
/// # Errors
/// When `at` falls inside none of them.
pub fn split(sequence: &mut EventSequence, ids: &[u64], at: f64) -> Result<Vec<u64>, String> {
    let beat_unit = Transcription::of(sequence).beat_unit;
    let mut made = Vec::new();
    for id in ids {
        let Some(i) = sequence.index(*id) else {
            continue;
        };
        let event = &sequence.events[i];
        let keys = event.keys();
        if Type::of(&keys) != Type::Note {
            continue;
        }
        let written = keys.contains_key("value");
        let sounding = render::sustain_of(&keys).max(0.0);
        let length = if written {
            keys.get("dur").and_then(Value::as_f64).unwrap_or(sounding)
        } else {
            sounding
        };
        let start = event.at.0;
        let first = at - start;
        if first <= EPSILON || first >= length - EPSILON {
            continue;
        }
        let second = length - first;
        let (mut a, mut b) = (keys.clone(), keys);
        if written {
            for (half, beats) in [(&mut a, first), (&mut b, second)] {
                if let Some(held) = half.get("sustain").and_then(Value::as_f64) {
                    half.insert("sustain".into(), json!(held * beats / length));
                }
                half.insert("dur".into(), json!(beats));
                match written_value(beats, beat_unit) {
                    Some(value) => half.insert("value".into(), value),
                    None => half.remove("value"),
                };
            }
        } else {
            let dur = a.get("dur").and_then(Value::as_f64);
            render::set_key(&mut a, "sustain", json!(first));
            render::set_key(&mut b, "sustain", json!(second));
            if let Some(dur) = dur {
                a.insert("dur".into(), json!(first));
                b.insert("dur".into(), json!((dur - first).max(second)));
            }
        }
        b.remove("joins");
        let curves = sequence.events[i].automation.clone();
        let (mut before, mut after) = (Vec::new(), Vec::new());
        for curve in curves {
            if curve.points.is_empty() {
                before.push(curve);
                continue;
            }
            let (head, tail) = cut(&curve.points, first);
            if !head.is_empty() {
                before.push(crate::multitrack::Automation {
                    points: head,
                    ..curve.clone()
                });
            }
            if !tail.is_empty() {
                let fresh = NodeId(sequence.mint());
                after.push(crate::multitrack::Automation {
                    id: fresh,
                    points: tail,
                    ..curve
                });
            }
        }
        let new = sequence.mint();
        let event = &mut sequence.events[i];
        event.data = Opaque(Value::Object(a));
        event.automation = before;
        let half = super::Event {
            id: new,
            at: Beat(at),
            data: Opaque(Value::Object(b)),
            automation: after,
        };
        for other in &mut sequence.events {
            let mut keys = other.keys();
            if keys.get("joins").and_then(Value::as_u64) == Some(*id) {
                keys.insert("joins".into(), json!(new));
                other.data = Opaque(Value::Object(keys));
            }
        }
        let place = sequence.events.partition_point(|e| e.at.0 <= at);
        sequence.events.insert(place, half);
        retarget_spanners(sequence, *id, new);
        made.push(new);
    }
    if made.is_empty() {
        return Err("the beat is inside none of these notes".into());
    }
    Ok(made)
}

/// A curve's points cut at `at`, beats from the note's start: those before,
/// and those after measured from the cut, each side ending or starting on the
/// value the curve has there.
fn cut(points: &[Point<Beat>], at: f64) -> (Vec<Point<Beat>>, Vec<Point<Beat>>) {
    let shifted = || {
        points
            .iter()
            .map(|p| Point {
                at: Beat(p.at.0 - at),
                ..p.clone()
            })
            .collect()
    };
    // A curve on one side of the cut goes whole with that side.
    if points.iter().all(|p| p.at.0 >= at) {
        return (Vec::new(), shifted());
    }
    if points.iter().all(|p| p.at.0 <= at) {
        return (points.to_vec(), Vec::new());
    }
    let there = value_at(points, Beat(at));
    let mut head: Vec<Point<Beat>> = points.iter().filter(|p| p.at.0 < at).cloned().collect();
    let mut tail: Vec<Point<Beat>> = points
        .iter()
        .filter(|p| p.at.0 > at)
        .map(|p| Point {
            at: Beat(p.at.0 - at),
            ..p.clone()
        })
        .collect();
    // The segment the cut falls in goes on in the second half with the shape
    // it had: the point it started from lends its data.
    let shaping = points
        .iter()
        .rev()
        .find(|p| p.at.0 <= at)
        .map(|p| p.data.clone())
        .unwrap_or_default();
    head.push(Point {
        at: Beat(at),
        value: there,
        data: Opaque::default(),
    });
    tail.insert(
        0,
        Point {
            at: Beat(0.0),
            value: there,
            data: shaping,
        },
    );
    (head, tail)
}

/// A spanner of the section that ended on `from` ends on `to`.
fn retarget_spanners(sequence: &mut EventSequence, from: u64, to: u64) {
    let Some(Value::Object(section)) = sequence.notation.as_mut() else {
        return;
    };
    let Some(Value::Array(list)) = section.get_mut("spanners") else {
        return;
    };
    for spanner in list {
        if spanner.get("to").and_then(Value::as_u64) == Some(from) {
            spanner["to"] = json!(to);
        }
    }
}
