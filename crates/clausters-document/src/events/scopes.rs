//! **A curve moved between its two scopes**: a channel's lane into the notes
//! it reaches, and the notes' curves back into a lane.
//!
//! A lane is a channel's function, and a note hears it from its on to its
//! off; a note's expression is the note's own. So a lane given to its notes
//! -- each one the stretch of the lane its span covers -- sounds exactly as
//! it did, and the note now carries, as part of what it is, the curve that
//! shaped it. That is the direction that always holds, and the lane goes:
//! kept beside the notes' copies, a bend would be heard twice.
//!
//! The other direction holds when the notes agree: where two of them sound
//! at once, one channel can only have said one value, so their curves must be
//! the same over the time they share -- a chord whose lane was given to its
//! notes gives it back. Where they differ it is refused and says which.

use serde_json::{Value, json};

use clausters_core::event::render;

use super::{CurveKind, EventSequence, MidiSpec};
use crate::multitrack::Automation;
use crate::multitrack::nodes::value_at;
use crate::{NodeId, Opaque, Point};

/// How close two values of one curve are to be the same.
const SAME: f64 = 1e-9;

/// A note's channel: its `channel` key, 0 when it names none.
fn channel_of(keys: &serde_json::Map<String, Value>) -> i64 {
    keys.get("channel").and_then(Value::as_f64).unwrap_or(0.0) as i64
}

/// A target without the channel a lane names -- what a note's curve over the
/// same control is written as.
fn without_channel(target: &Value) -> Value {
    let mut target = target.clone();
    if let Some(map) = target.as_object_mut() {
        map.remove("channel");
    }
    target
}

/// **A lane's stretch from beat `from` to `to`, as a curve of its own** that
/// starts at 0: the value where it begins -- shaped as the segment it falls
/// in -- every point inside, and the value where it ends.
fn stretch(points: &[Point], from: f64, to: f64) -> Vec<Point> {
    let shape_at = |at: f64| {
        points
            .iter()
            .rev()
            .find(|p| p.at <= at)
            .map_or(Opaque::none(), |p| p.data.clone())
    };
    let mut out = vec![Point {
        at: 0.0,
        value: value_at(points, from),
        data: shape_at(from),
    }];
    out.extend(
        points
            .iter()
            .filter(|p| p.at > from && p.at < to)
            .map(|p| Point {
                at: p.at - from,
                ..p.clone()
            }),
    );
    if to > from {
        out.push(Point {
            at: to - from,
            value: value_at(points, to),
            data: shape_at(to),
        });
    }
    out
}

impl EventSequence {
    /// **Lane `lane` given to the notes it reaches**: each note on its channel
    /// (every note, for a lane that names none) takes the stretch of the lane
    /// its span covers as a curve of its own, and the lane goes. A note with a
    /// curve of its own over the same control keeps it -- it was already what
    /// the note heard -- unless the control is the bend, which adds and so
    /// would be heard twice: that is refused. So is a lane the sequence's
    /// spec cannot say of one note.
    pub(super) fn lane_to_expression(&mut self, lane: NodeId) -> Result<(), String> {
        let index = self
            .lanes
            .iter()
            .position(|l| l.id == lane)
            .ok_or_else(|| format!("the sequence holds no lane {}", lane.0))?;
        let held = self.lanes[index].clone();
        let kind = CurveKind::of(&held.target.0);
        if let Some(spec) = self.midi
            && !spec.says_note(&without_channel(&held.target.0))
        {
            return Err(format!(
                "the lane cannot be the notes' own: {}",
                refusal(spec, &held.target.0)
            ));
        }
        let channel = held.target.0.get("channel").and_then(Value::as_f64);
        let target = without_channel(&held.target.0);
        let reached: Vec<usize> = self
            .events
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                let keys = e.keys();
                render::Type::of(&keys) == render::Type::Note
                    && channel.is_none_or(|c| c as i64 == channel_of(&keys))
            })
            .map(|(i, _)| i)
            .collect();
        for &i in &reached {
            let own = self.events[i]
                .expression
                .iter()
                .any(|c| CurveKind::of(&c.target.0) == kind);
            if own && kind == CurveKind::Bend {
                return Err(format!(
                    "event {} has a bend of its own, and a bend adds: the lane given to it would be heard twice",
                    self.events[i].id
                ));
            }
        }
        for i in reached {
            if self.events[i]
                .expression
                .iter()
                .any(|c| CurveKind::of(&c.target.0) == kind)
            {
                continue;
            }
            let at = self.events[i].at.0;
            let sustain = self.events[i].sustain().max(0.0);
            let id = NodeId(self.mint());
            let mut curve = Automation::new(id, Opaque(target.clone()));
            curve.name = held.name.clone();
            curve.points = stretch(&held.points, at, at + sustain);
            self.events[i].expression.push(curve);
        }
        self.lanes.remove(index);
        Ok(())
    }

    /// **The notes' curves over `target` gathered into a lane** -- of the
    /// notes on `channel`, or of every note -- and answered by the lane's id:
    /// each note's curve over its span, in the sequence's beats. Refused where
    /// two notes that sound at once have different curves over the time they
    /// share, since one channel can only say one value; the lane names the
    /// channel its notes share, if they share one.
    pub(super) fn expression_to_lane(
        &mut self,
        target: &Value,
        channel: Option<i64>,
    ) -> Result<u64, String> {
        let kind = CurveKind::of(target);
        if let Some(spec) = self.midi
            && !spec.says_lane(target)
        {
            return Err(refusal(spec, target));
        }
        // Each note that has one: its event index, its span and its curve.
        let mut spans: Vec<(usize, f64, f64, Vec<Point>)> = Vec::new();
        for (i, event) in self.events.iter().enumerate() {
            let keys = event.keys();
            if render::Type::of(&keys) != render::Type::Note
                || channel.is_some_and(|c| c != channel_of(&keys))
            {
                continue;
            }
            let Some(curve) = event
                .expression
                .iter()
                .find(|c| CurveKind::of(&c.target.0) == kind)
            else {
                continue;
            };
            let at = event.at.0;
            let absolute = curve
                .points
                .iter()
                .map(|p| Point {
                    at: at + p.at,
                    ..p.clone()
                })
                .collect();
            spans.push((i, at, at + event.sustain().max(0.0), absolute));
        }
        if spans.is_empty() {
            return Err("no note has a curve over that control".into());
        }
        // Where two sound at once, their curves must agree: at every point
        // of either inside the time they share, and at its ends.
        for (a, first) in spans.iter().enumerate() {
            for second in &spans[a + 1..] {
                let (from, to) = (first.1.max(second.1), first.2.min(second.2));
                if from >= to {
                    continue;
                }
                let mut times: Vec<f64> = vec![from, to, (from + to) / 2.0];
                times.extend(
                    first
                        .3
                        .iter()
                        .chain(&second.3)
                        .map(|p| p.at)
                        .filter(|t| *t > from && *t < to),
                );
                let differs = times.iter().any(|&t| {
                    let (x, y) = (value_at(&first.3, t), value_at(&second.3, t));
                    (x - y).abs() > SAME * x.abs().max(y.abs()).max(1.0)
                });
                if differs {
                    return Err(format!(
                        "events {} and {} sound at once with different curves: one channel cannot say both",
                        self.events[first.0].id, self.events[second.0].id
                    ));
                }
            }
        }
        let channels: Vec<i64> = spans
            .iter()
            .map(|(i, ..)| channel_of(&self.events[*i].keys()))
            .collect();
        let shared = channel.or_else(|| {
            channels
                .iter()
                .all(|c| *c == channels[0])
                .then(|| channels[0])
        });
        let mut points: Vec<Point> = spans
            .iter()
            .flat_map(|(_, from, to, curve)| {
                curve
                    .iter()
                    .filter(|p| p.at >= *from - SAME && p.at <= *to + SAME)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .collect();
        points.sort_by(|a, b| a.at.total_cmp(&b.at));
        points.dedup_by(|b, a| (a.at - b.at).abs() <= SAME && (a.value - b.value).abs() <= SAME);
        let name = spans.iter().find_map(|(i, ..)| {
            self.events[*i]
                .expression
                .iter()
                .find(|c| CurveKind::of(&c.target.0) == kind)
                .and_then(|c| c.name.clone())
        });
        for (i, ..) in &spans {
            self.events[*i]
                .expression
                .retain(|c| CurveKind::of(&c.target.0) != kind);
        }
        let mut lane_target = target.clone();
        if let (Some(c), Some(map)) = (shared, lane_target.as_object_mut()) {
            map.insert("channel".into(), json!(c));
        }
        let id = self.mint();
        let mut lane = Automation::new(NodeId(id), Opaque(lane_target));
        lane.name = name;
        lane.points = points;
        self.lanes.push(lane);
        Ok(id)
    }
}

/// Why `target` cannot be said where the spec asks.
fn refusal(spec: MidiSpec, target: &Value) -> String {
    format!(
        "{} has no {} spelling over one note",
        match CurveKind::of(target) {
            CurveKind::Cc(n) => format!("CC {n}"),
            CurveKind::Bend => "a bend".into(),
            CurveKind::Pressure => "pressure".into(),
            CurveKind::Timbre => "timbre".into(),
            CurveKind::Control => "a control".into(),
        },
        spec.label()
    )
}
