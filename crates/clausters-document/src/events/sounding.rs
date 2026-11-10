//! **The pitch a sequence sounds over time**, line by line: what a roll draws
//! over its boxes and what a score can read of a line played without a new
//! attack.
//!
//! A **run** is the notes one line plays from an attack until the next: a
//! note no other goes on from starts one, and a note whose `joins` names it
//! carries it on. Over each note the run is the note's pitch (fractional MIDI
//! numbers, as the note resolves it) plus what bends it: the note's own bend
//! curve, measured from its start, and the bend of its channel, a curve of the
//! sequence on its beats -- both in semitones, and added, as the playback adds
//! them. A note sounds to its end or to where the note it goes on into starts,
//! whichever is first, so a legato played long hands over without running
//! backwards. A segment a curve bends rather than draws straight is read at a
//! few points between its ends, so the run follows the shape that sounds.
//!
//! It is a reading only: a channel's bend stays its own curve, and nothing
//! here is written back.

use std::collections::BTreeMap;

use serde_json::Value;

use clausters_core::envshape::SHAPE_LINEAR;
use clausters_core::event::render::{self, Type};

use super::{CurveKind, EventSequence};
use crate::Point;
use crate::multitrack::nodes::value_at;
use crate::timebase::Beat;

/// The points a bent segment is read at, between its ends.
const BENT_STEPS: usize = 8;

/// **One run**: the notes it carries, in the order they sound, and the pitch
/// over time as `[beat, midinote]` points, in beat order.
#[derive(Debug, Clone, PartialEq)]
pub struct PitchRun {
    /// The ids of its notes.
    pub notes: Vec<u64>,
    /// The pitch it sounds: `[beat, fractional MIDI number]`.
    pub points: Vec<[f64; 2]>,
}

/// What a note brings to a run.
struct Sounding<'a> {
    id: u64,
    at: f64,
    end: f64,
    midi: f64,
    own: Option<&'a [Point<Beat>]>,
    channel: Option<&'a [Point<Beat>]>,
}

/// Whether the segment of `points` that holds `at` is drawn other than
/// straight.
fn bent(points: &[Point<Beat>], at: f64) -> bool {
    points
        .iter()
        .rev()
        .find(|p| p.at.0 <= at)
        .and_then(|p| p.data.0.get("shape"))
        .and_then(Value::as_f64)
        .is_some_and(|s| s as i32 != SHAPE_LINEAR)
}

impl Sounding<'_> {
    /// The pitch at beat `t`.
    fn at_beat(&self, t: f64) -> f64 {
        let own = self
            .own
            .map_or(0.0, |points| value_at(points, Beat(t - self.at)));
        let channel = self.channel.map_or(0.0, |points| value_at(points, Beat(t)));
        self.midi + own + channel
    }

    /// Whether either curve bends the segment that starts at `t`.
    fn bends_after(&self, t: f64) -> bool {
        self.own.is_some_and(|p| bent(p, t - self.at)) || self.channel.is_some_and(|p| bent(p, t))
    }

    /// The points it sounds from its start to `until`.
    fn points(&self, until: f64, out: &mut Vec<[f64; 2]>) {
        let mut beats = vec![self.at, until];
        if let Some(points) = self.own {
            beats.extend(points.iter().map(|p| self.at + p.at.0));
        }
        if let Some(points) = self.channel {
            beats.extend(points.iter().map(|p| p.at.0));
        }
        beats.retain(|b| *b >= self.at && *b <= until);
        beats.sort_by(f64::total_cmp);
        beats.dedup_by(|a, b| (*a - *b).abs() < 1e-12);
        for pair in beats.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            out.push([a, self.at_beat(a)]);
            if self.bends_after(a) {
                for step in 1..BENT_STEPS {
                    let t = a + (b - a) * step as f64 / BENT_STEPS as f64;
                    out.push([t, self.at_beat(t)]);
                }
            }
        }
        if let Some(last) = beats.last() {
            out.push([*last, self.at_beat(*last)]);
        }
    }
}

/// The first bend of `curves` that has points.
fn first_bend<'a>(
    curves: impl IntoIterator<Item = &'a crate::multitrack::Automation<Beat>>,
) -> Option<&'a [Point<Beat>]> {
    curves
        .into_iter()
        .find(|c| CurveKind::of(&c.target.0) == CurveKind::Bend && !c.points.is_empty())
        .map(|c| c.points.as_slice())
}

/// **The sequence's runs** (see the module), each from the note that attacks
/// it, in the order they start.
#[must_use]
pub fn pitch_runs(sequence: &EventSequence) -> Vec<PitchRun> {
    let mut notes: BTreeMap<u64, Sounding> = BTreeMap::new();
    let mut joins: BTreeMap<u64, u64> = BTreeMap::new();
    for event in &sequence.events {
        let keys = event.keys();
        if Type::of(&keys) != Type::Note {
            continue;
        }
        let channel = keys.get("channel").and_then(Value::as_f64).unwrap_or(0.0) as i64;
        let reaching = sequence.automation.iter().filter(|c| {
            c.target
                .0
                .get("channel")
                .and_then(Value::as_f64)
                .is_none_or(|n| n as i64 == channel)
        });
        let at = event.at.0;
        notes.insert(
            event.id,
            Sounding {
                id: event.id,
                at,
                end: at + render::sustain_of(&keys).max(0.0),
                midi: render::pitch_of(&keys).midinote(&render::scale_of(&keys)),
                own: first_bend(&event.automation),
                channel: first_bend(reaching),
            },
        );
        if let Some(from) = keys.get("joins").and_then(Value::as_u64) {
            joins.insert(event.id, from);
        }
    }
    // what goes on from each note, by onset
    let mut next: BTreeMap<u64, Vec<(f64, u64)>> = BTreeMap::new();
    for (into, from) in &joins {
        if notes.contains_key(from) {
            next.entry(*from).or_default().push((notes[into].at, *into));
        }
    }
    for list in next.values_mut() {
        list.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    }
    // a run starts at a note nothing carries on into: one no join reaches,
    // or one that is not the first a note goes on into
    let carried: std::collections::BTreeSet<u64> = next
        .values()
        .filter_map(|l| l.first())
        .map(|(_, id)| *id)
        .collect();
    let mut starts: Vec<&Sounding> = notes
        .values()
        .filter(|n| !carried.contains(&n.id))
        .collect();
    starts.sort_by(|a, b| a.at.total_cmp(&b.at).then(a.id.cmp(&b.id)));
    let mut runs = Vec::new();
    for start in starts {
        let mut run = PitchRun {
            notes: Vec::new(),
            points: Vec::new(),
        };
        let mut note = start;
        loop {
            run.notes.push(note.id);
            let then = next
                .get(&note.id)
                .and_then(|l| l.first())
                .map(|(_, id)| &notes[id])
                .filter(|n| !run.notes.contains(&n.id));
            let until = match then {
                Some(then) if then.at > note.at => note.end.min(then.at),
                _ => note.end,
            };
            note.points(until, &mut run.points);
            match then {
                Some(then) => note = then,
                None => break,
            }
        }
        runs.push(run);
    }
    runs
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::events::Event;
    use crate::multitrack::Automation;
    use crate::{NodeId, Opaque};

    fn note(id: u64, at: f64, midinote: f64, dur: f64, extra: Value) -> Event {
        let mut keys = json!({"type": "note", "midinote": midinote, "dur": dur, "legato": 1.0});
        if let (Some(keys), Some(extra)) = (keys.as_object_mut(), extra.as_object()) {
            keys.extend(extra.clone());
        }
        Event {
            id,
            at: Beat(at),
            data: Opaque(keys),
            automation: Vec::new(),
        }
    }

    fn bend(points: &[(f64, f64)]) -> Automation<Beat> {
        let mut curve = Automation::new(NodeId(90), Opaque(json!({"bend": true})));
        curve.points = points
            .iter()
            .map(|&(at, value)| Point {
                at: Beat(at),
                value,
                data: Opaque::default(),
            })
            .collect();
        curve
    }

    /// **Two joined notes with a bend are one run that follows it**, and an
    /// unjoined third starts another.
    #[test]
    fn joined_notes_are_one_run_and_an_attack_starts_another() {
        let mut first = note(1, 0.0, 60.0, 1.0, json!({}));
        first.automation.push(bend(&[(0.0, 0.0), (1.0, 2.0)]));
        let sequence = EventSequence {
            events: vec![
                first,
                note(2, 1.0, 64.0, 1.0, json!({"joins": 1})),
                note(3, 2.0, 67.0, 1.0, json!({})),
            ],
            ..EventSequence::default()
        };
        let runs = pitch_runs(&sequence);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].notes, vec![1, 2]);
        assert_eq!(
            runs[0].points,
            vec![[0.0, 60.0], [1.0, 62.0], [1.0, 64.0], [2.0, 64.0]]
        );
        assert_eq!(runs[1].notes, vec![3]);
        assert_eq!(runs[1].points, vec![[2.0, 67.0], [3.0, 67.0]]);
    }

    /// **A channel's bend adds to the note's**, over the time the note
    /// sounds, and a note on another channel does not hear it.
    #[test]
    fn a_channel_bend_adds_over_the_notes_on_its_channel() {
        let mut channel = bend(&[(0.0, 0.0), (2.0, -2.0)]);
        channel.target = Opaque(json!({"bend": true, "channel": 0}));
        let sequence = EventSequence {
            events: vec![
                note(1, 0.0, 60.0, 2.0, json!({})),
                note(2, 0.0, 72.0, 2.0, json!({"channel": 1})),
            ],
            automation: vec![channel],
            ..EventSequence::default()
        };
        let runs = pitch_runs(&sequence);
        assert_eq!(runs[0].points, vec![[0.0, 60.0], [2.0, 58.0]]);
        assert_eq!(runs[1].points, vec![[0.0, 72.0], [2.0, 72.0]]);
    }

    /// **A legato played long hands over where the next note starts**, and
    /// a bent segment is read between its ends.
    #[test]
    fn an_overlap_hands_over_and_a_bent_segment_is_followed() {
        let mut first = note(1, 0.0, 60.0, 2.0, json!({}));
        let mut curve = bend(&[(0.0, 0.0), (2.0, 4.0)]);
        curve.points[0].data = Opaque(json!({"shape": 5, "curve": 3.0}));
        first.automation.push(curve);
        let sequence = EventSequence {
            events: vec![first, note(2, 1.0, 62.0, 1.0, json!({"joins": 1}))],
            ..EventSequence::default()
        };
        let run = &pitch_runs(&sequence)[0];
        let handed = run.points.iter().position(|p| p[1] == 62.0).unwrap();
        assert!(run.points[..handed].iter().all(|p| p[0] <= 1.0));
        assert!(handed > 2 + BENT_STEPS / 2, "{:?}", run.points);
    }
}
