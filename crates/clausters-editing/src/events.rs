//! **What a gesture over a timeline of events means.**
//!
//! A roll draws two lanes over one structure — the notes, and the OSC and MIDI
//! markers beside them — and reports each as its own flat list. Both state the
//! *whole* lane, so what comes out is the timeline as it now stands, notes and
//! markers together, in one `setevents`.
//!
//! # What an event is stays the client's, and that is what made this movable
//!
//! The `events` vocabulary carries an event's `data` and never reads it: a
//! note's instrument, an `OscItem`'s arguments, whatever else an author put on
//! it, all travel whole and come back whole. So the ingestion never needs the
//! client's objects — it needs the timeline *as the vocabulary already holds
//! it*, which is what both clients were already computing to ask
//! `domain::edit` for an inverse. The one thing it has to be able to tell apart
//! is a note from a marker, and the data says which: a marker names itself with
//! its own key, `osc` or `midi`, which is the same rule
//! [`clausters_document`]'s clang configuration reads them by.
//!
//! # The two lanes are two gestures, not two structures
//!
//! Which is why the lane a hand did *not* touch is carried through untouched
//! rather than left out. A payload that stated only the notes would be an edit
//! that deletes every marker, and its inverse would put them back — a pile of
//! entries that each undo a loss nobody made.

use std::collections::HashSet;

use serde_json::{Map, Value, json};

use crate::intake::{Intake, groups, number, text};

/// What the `pianoroll` widget sends and takes per note: start, duration,
/// pitch, velocity, channel.
pub const QUINTUPLE: usize = 5;

/// And per marker: the time and the label.
pub const PAIR: usize = 2;

/// The default velocity of a note that states neither one nor an amplitude.
const DEFAULT_VELOCITY: i64 = 100;

/// The label the roll's marker lane draws an item with, or `None` when the item
/// is not one of that lane's.
///
/// An OSC marker labels with its address, because **a marker is the message it
/// sends** and the address is the whole of what a roll can show of one; a MIDI
/// item labels with a short tag.
pub fn label_of(data: &Value) -> Option<String> {
    let data = data.as_object()?;
    if let Some(addr) = data.get("osc") {
        return Some(text(addr));
    }
    data.contains_key("midi").then(|| "midi".to_string())
}

/// The MIDI velocity a note is drawn at: an explicit `velocity`, else the
/// linear `amp` mapped onto the velocity range, else the default.
pub fn velocity_of(data: &Value) -> i64 {
    let Some(data) = data.as_object() else {
        return DEFAULT_VELOCITY;
    };
    if let Some(velocity) = data.get("velocity").and_then(Value::as_f64) {
        return (velocity as i64).clamp(0, 127);
    }
    if let Some(amp) = data.get("amp").and_then(Value::as_f64) {
        return (amp * 127.0).round().clamp(1.0, 127.0) as i64;
    }
    DEFAULT_VELOCITY
}

/// What an item of the timeline *is*, as the vocabulary holds it.
fn data(event: &Value) -> &Value {
    event.get("data").unwrap_or(&Value::Null)
}

fn event(beat: f64, params: Value) -> Value {
    json!({ "at": beat, "data": params })
}

/// The items the gesture did **not** draw, in the order the timeline holds
/// them — what keeps the lane nobody touched out of the edit that rebuilt the
/// other one, and out of the inverse that puts it back.
fn kept(state: &[Value], drew_markers: bool) -> Vec<Value> {
    state
        .iter()
        .filter(|item| label_of(data(item)).is_some() != drew_markers)
        .cloned()
        .collect()
}

/// The whole timeline after a `notes` gesture: the drawn notes, with every
/// marker left exactly where it is.
fn notes(state: &[Value], values: &[Value], units_per_beat: f64) -> Vec<Value> {
    let held: Vec<&Value> = state
        .iter()
        .filter(|item| label_of(data(item)).is_none())
        .collect();
    let mut out = Vec::new();
    for (i, quintuple) in groups(values, QUINTUPLE).enumerate() {
        let (start, dur) = (number(&quintuple[0]), number(&quintuple[1]));
        let (pitch, velocity) = (number(&quintuple[2]) as i64, number(&quintuple[3]) as i64);
        let channel = number(&quintuple[4]) as i64;
        let length = dur / units_per_beat;
        let mut params = match held.get(i) {
            // **An edit updates the note it names; it does not rebuild it.**
            // Order is the only identity the payload carries, so the i-th
            // note's own event is copied and the drawn fields written over it —
            // which keeps the instrument and everything else the author put
            // there.
            Some(was) => {
                let mut params = data(was).as_object().cloned().unwrap_or_default();
                params.insert("midinote".into(), json!(pitch));
                params.insert("sustain".into(), json!(length));
                if velocity != velocity_of(data(was)) {
                    params.insert("velocity".into(), json!(velocity));
                    params.insert("amp".into(), json!(amp_of(velocity)));
                }
                params
            }
            None => {
                let mut params = Map::new();
                params.insert("midinote".into(), json!(pitch));
                params.insert("dur".into(), json!(length));
                params.insert("legato".into(), json!(1.0));
                params.insert("amp".into(), json!(amp_of(velocity)));
                params.insert("velocity".into(), json!(velocity));
                params
            }
        };
        if channel != 0 {
            params.insert("channel".into(), json!(channel));
        }
        out.push(event(start / units_per_beat, Value::Object(params)));
    }
    out.extend(kept(state, false));
    out
}

/// A velocity as the linear amplitude that goes with it.
fn amp_of(velocity: i64) -> f64 {
    (velocity as f64 / 127.0).clamp(0.0, 1.0)
}

/// The whole timeline after an `osc` gesture — the notes untouched and the
/// markers as the lane now holds them — or `None` when the gesture added one
/// that has no message to send.
///
/// **A marker is matched by its label**, which is its address, and only then by
/// order among the ones that share it. The report carries the label the lane
/// drew, so the message a marker sends survives being dragged and — unlike the
/// notes one lane up, where order is the only identity there is — survives a
/// *neighbour* being removed as well.
fn markers(state: &[Value], values: &[Value], units_per_beat: f64) -> Option<Vec<Value>> {
    let held: Vec<&Value> = state
        .iter()
        .filter(|item| label_of(data(item)).is_some())
        .collect();
    let mut taken: HashSet<usize> = HashSet::new();
    let mut out = kept(state, true);
    for pair in groups(values, PAIR) {
        let (time, label) = (number(&pair[0]), text(&pair[1]));
        let was = held.iter().enumerate().find_map(|(i, item)| {
            (!taken.contains(&i) && label_of(data(item)).as_deref() == Some(&label)).then_some(i)
        })?;
        taken.insert(was);
        out.push(event(time / units_per_beat, data(held[was]).clone()));
    }
    Some(out)
}

/// **What a gesture over a timeline means**, in the `events` vocabulary.
///
/// `state` is the timeline as the vocabulary holds it — every item, notes and
/// markers alike, since both are edited through it. `units_per_beat` is what a
/// beat is worth on the view's axis, because a roll draws in timeline samples
/// and a timeline is in beats.
///
/// `editable` is `false` for a roll over what a **generator** produced: that is
/// a rendering of an algorithm, so there is nothing to write it onto.
pub fn intake(
    state: &[Value],
    tag: &str,
    values: &[Value],
    units_per_beat: f64,
    editable: bool,
) -> Intake {
    if !editable {
        return Intake::nothing();
    }
    let units_per_beat = if units_per_beat == 0.0 {
        1.0
    } else {
        units_per_beat
    };
    match tag {
        "notes" => Intake::edit(
            json!({ "intent": "setevents", "events": notes(state, values, units_per_beat) }),
            "edit the notes",
        ),
        "osc" => match markers(state, values, units_per_beat) {
            Some(events) => Intake::edit(
                json!({ "intent": "setevents", "events": events }),
                "edit the markers",
            ),
            // A marker *is* the message it sends, and a roll has no way to type
            // one — so a marker added there has nothing to become. Saying so is
            // the point: a picture that springs back with nothing attached
            // teaches "sometimes it does not work" rather than "not here".
            // **The sentence names no language.** It is one string in the
            // crate now, so a page showing Python's spelling of `add` — or a
            // script showing the page's — would be this milestone putting a
            // divergence *into* both clients instead of taking one out.
            None => Intake::refused(
                "a marker is the message it sends, and a roll cannot say which: \
                 add it to the timeline with its address, then drag it here",
            ),
        },
        _ => Intake::nothing(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(beat: f64, pitch: i64) -> Value {
        json!({ "at": beat, "data": { "midinote": pitch, "instrument": "bell" } })
    }

    fn marker(beat: f64, addr: &str) -> Value {
        json!({ "at": beat, "data": { "osc": addr, "args": [1] } })
    }

    /// A note that only moved keeps everything the roll cannot draw.
    #[test]
    fn an_edited_note_is_updated_and_not_rebuilt() {
        let state = vec![note(0.0, 60)];
        let taken = intake(
            &state,
            "notes",
            &[json!(2.0), json!(1.0), json!(64), json!(100), json!(0)],
            1.0,
            true,
        );
        let events = taken.payloads[0]["events"].as_array().expect("events");
        assert_eq!(events[0]["at"], json!(2.0));
        assert_eq!(events[0]["data"]["midinote"], json!(64));
        assert_eq!(events[0]["data"]["instrument"], json!("bell"));
    }

    /// The lane the hand did not touch travels through untouched.
    #[test]
    fn the_other_lane_is_carried_and_not_dropped() {
        let state = vec![note(0.0, 60), marker(1.0, "/cue")];
        let drawn = intake(
            &state,
            "notes",
            &[json!(0.0), json!(1.0), json!(60), json!(100), json!(0)],
            1.0,
            true,
        );
        let events = drawn.payloads[0]["events"].as_array().expect("events");
        assert_eq!(events.len(), 2);
        assert_eq!(events[1]["data"]["osc"], json!("/cue"));

        let dragged = intake(&state, "osc", &[json!(3.0), json!("/cue")], 1.0, true);
        let events = dragged.payloads[0]["events"].as_array().expect("events");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["data"]["midinote"], json!(60));
        assert_eq!(events[1]["at"], json!(3.0));
        assert_eq!(dragged.label, "edit the markers");
    }

    /// A marker the roll invented has no message, and that is said rather than
    /// silently dropped.
    #[test]
    fn a_marker_with_no_message_is_refused_out_loud() {
        let state = vec![marker(1.0, "/cue")];
        let added = intake(
            &state,
            "osc",
            &[json!(1.0), json!("/cue"), json!(2.0), json!("")],
            1.0,
            true,
        );
        assert!(added.payloads.is_empty());
        assert!(added.refusal.is_some());
    }

    /// A roll over what a generator produced writes onto nothing.
    #[test]
    fn a_generated_roll_is_not_written_back() {
        let state = vec![note(0.0, 60)];
        assert_eq!(
            intake(
                &state,
                "notes",
                &[json!(0.0), json!(1.0), json!(62), json!(100), json!(0)],
                1.0,
                false
            ),
            Intake::nothing()
        );
    }

    /// The axis crossing is here: the roll draws in samples, the timeline is in
    /// beats.
    #[test]
    fn the_roll_s_axis_is_crossed_into_beats() {
        let state: Vec<Value> = Vec::new();
        let taken = intake(
            &state,
            "notes",
            &[json!(960.0), json!(480.0), json!(60), json!(100), json!(0)],
            480.0,
            true,
        );
        let events = taken.payloads[0]["events"].as_array().expect("events");
        assert_eq!(events[0]["at"], json!(2.0));
        assert_eq!(events[0]["data"]["dur"], json!(1.0));
    }
}
