//! **The curves a roll adds and takes away**, among those the sequence's
//! format allows.
//!
//! A channel's curve is a lane under the plane; a note's is drawn against
//! its box. A curve is added with one point, at the parameter's resting
//! value, and the hand draws the rest; which targets a format can say is
//! the document's ([`clausters_document::events::MidiSpec`]), which refuses
//! the rest with why -- under MPE a channel says its controllers, and a note
//! its own bend, pressure and timbre.
//!
//! - `curve_cc1`, `curve_cc11`, `curve_bend`, `curve_pressure`: a curve of
//!   the sequence on the channel of the first selected note (the first
//!   channel with none selected): the modulation wheel, the expression, the
//!   bend, the channel's pressure.
//! - `note_bend`, `note_pressure`, `note_timbre`: a curve of each selected
//!   note that has none of its kind.
//! - `curve_remove` with a curve's name: that curve, the sequence's or a
//!   note's, taken away.

use serde_json::{Value, json};

use clausters_core::event::render::Type;
use clausters_document::events::{CurveKind, EventSequence, EventsIntent};
use clausters_document::multitrack::Automation;
use clausters_document::{Beat, NodeId, Opaque, Point};

use crate::intake::{Intake, number, text};

/// The roll's verbs over curves.
pub const CURVE_VERBS: &[&str] = &[
    "curve_cc1",
    "curve_cc11",
    "curve_bend",
    "curve_pressure",
    "note_bend",
    "note_pressure",
    "note_timbre",
    "curve_remove",
];

/// What a verb adds: its target, without the channel, and the value it
/// rests at.
fn target_of(verb: &str) -> Option<(Value, f64)> {
    Some(match verb {
        "curve_cc1" => (json!({"cc": 1}), 0.0),
        "curve_cc11" => (json!({"cc": 11}), 127.0),
        "curve_bend" | "note_bend" => (json!({"bend": true}), 0.0),
        "curve_pressure" | "note_pressure" => (json!({"pressure": true}), 0.0),
        "note_timbre" => (json!({"timbre": true}), 0.5),
        _ => return None,
    })
}

/// A curve over `target`, at `value` from its start.
fn curve(target: Value, value: f64) -> Automation<Beat> {
    let mut curve = Automation::new(NodeId(0), Opaque(target));
    curve.points = vec![Point {
        at: Beat(0.0),
        value,
        data: Opaque::default(),
    }];
    curve
}

/// **What a curve verb of the roll means** (see the module): the edit, or
/// why not; nothing for a tag that is none of them.
pub fn intake(sequence: &EventSequence, tag: &str, values: &[Value]) -> Intake {
    if tag == "curve_remove" {
        let name = values.first().map(text).unwrap_or_default();
        let Ok(id) = name.parse::<u64>() else {
            return Intake::refused("no curve is shown");
        };
        let curve = NodeId(id);
        if sequence.automation.iter().any(|c| c.id == curve) {
            let intent = EventsIntent::RemoveAutomation { curve };
            return Intake::edit(json!(intent), "remove a curve");
        }
        let held = sequence
            .events
            .iter()
            .find(|e| e.automation.iter().any(|c| c.id == curve));
        return match held {
            Some(event) => Intake::edit(
                json!(EventsIntent::RemoveEventAutomation {
                    id: event.id,
                    curve
                }),
                "remove a curve",
            ),
            None => Intake::refused("the sequence holds no such curve"),
        };
    }
    let Some((target, value)) = target_of(tag) else {
        return Intake::nothing();
    };
    let ids: Vec<u64> = values.iter().map(|v| number(v).max(0.0) as u64).collect();
    let notes: Vec<_> = ids
        .iter()
        .filter_map(|id| sequence.get(*id))
        .filter(|e| Type::of(&e.keys()) == Type::Note)
        .collect();
    if tag.starts_with("curve_") {
        let channel = notes
            .first()
            .and_then(|e| e.keys().get("channel").and_then(Value::as_f64))
            .unwrap_or(0.0);
        let mut target = target;
        target["channel"] = json!(channel);
        let kind = CurveKind::of(&target);
        let held = sequence.automation.iter().any(|c| {
            CurveKind::of(&c.target.0) == kind
                && c.target.0.get("channel").and_then(Value::as_f64) == Some(channel)
        });
        if held {
            return Intake::refused("the channel has that curve already");
        }
        let intent = EventsIntent::Automation {
            automation: curve(target, value),
        };
        return Intake::edit(json!(intent), "add a curve");
    }
    if notes.is_empty() {
        return Intake::refused("nothing is selected");
    }
    let kind = CurveKind::of(&target);
    let wanting: Vec<u64> = notes
        .iter()
        .filter(|e| {
            !e.automation
                .iter()
                .any(|c| CurveKind::of(&c.target.0) == kind)
        })
        .map(|e| e.id)
        .collect();
    if wanting.is_empty() {
        return Intake::refused("every selected note has that curve already");
    }
    // **one entry for every note**: the curves added to a copy, which the
    // edit puts in place whole
    let mut after = sequence.clone();
    for id in wanting {
        let intent = EventsIntent::EventAutomation {
            id,
            automation: curve(target.clone(), value),
        };
        if let Err(why) = after.edit(intent) {
            return Intake::refused(why);
        }
    }
    let intent = EventsIntent::Restore {
        sequence: Box::new(after),
    };
    Intake::edit(json!(intent), "add a curve")
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_document::events::{Event, MidiSpec};

    fn sequence(spec: Option<MidiSpec>) -> EventSequence {
        let mut s = EventSequence::new(vec![
            Event::new(0.0, json!({"midinote": 60, "sustain": 1.0, "channel": 1})),
            Event::new(1.0, json!({"midinote": 64, "sustain": 1.0, "channel": 2})),
        ]);
        s.midi = spec;
        s
    }

    fn applied(s: &EventSequence, intake: &Intake) -> Result<EventSequence, String> {
        if let Some(why) = &intake.refusal {
            return Err(why.clone());
        }
        let mut after = s.clone();
        for payload in &intake.payloads {
            after.edit(serde_json::from_value(payload.clone()).unwrap())?;
        }
        Ok(after)
    }

    /// **Under MPE a note's timbre is added and a channel's bend refused**,
    /// with why; a channel's controller is the master channel's.
    #[test]
    fn under_mpe_a_notes_timbre_is_added_and_a_channel_bend_refused() {
        let s = sequence(Some(MidiSpec::Mpe {
            upper: false,
            members: 15,
        }));
        let ids: Vec<Value> = s.events.iter().map(|e| json!(e.id)).collect();
        let timbre = applied(&s, &intake(&s, "note_timbre", &ids[..1])).unwrap();
        assert_eq!(timbre.events[0].automation.len(), 1);
        assert_eq!(
            timbre.events[0].automation[0].target.0,
            json!({"timbre": true})
        );
        let bend = applied(&s, &intake(&s, "curve_bend", &ids[..1]));
        assert!(bend.is_err(), "{bend:?}");
        let cc = applied(&s, &intake(&s, "curve_cc1", &[])).unwrap();
        assert_eq!(cc.automation[0].target.0, json!({"cc": 1, "channel": 0.0}));
    }

    /// **A curve is taken away by its name**, the sequence's or a note's, and
    /// a note that has one of the kind is left as it is.
    #[test]
    fn a_curve_is_removed_by_its_name_and_not_added_twice() {
        let s = sequence(None);
        let ids: Vec<Value> = s.events.iter().map(|e| json!(e.id)).collect();
        let bent = applied(&s, &intake(&s, "note_bend", &ids)).unwrap();
        assert!(bent.events.iter().all(|e| e.automation.len() == 1));
        assert!(intake(&bent, "note_bend", &ids).refusal.is_some());
        let name = bent.events[0].automation[0].id.0.to_string();
        let gone = applied(&bent, &intake(&bent, "curve_remove", &[json!(name)])).unwrap();
        assert!(gone.events[0].automation.is_empty());
        let lane = applied(&s, &intake(&s, "curve_cc11", &ids[1..])).unwrap();
        assert_eq!(lane.automation[0].target.0["channel"], json!(2.0));
        let name = lane.automation[0].id.0.to_string();
        let none = applied(&lane, &intake(&lane, "curve_remove", &[json!(name)])).unwrap();
        assert!(none.automation.is_empty());
    }
}
