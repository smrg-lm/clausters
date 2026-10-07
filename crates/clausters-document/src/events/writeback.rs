//! **An edit on the page, written back to the sequence it was read from.**
//!
//! A sequence read into a score ([`read`](super::transcription::read)) is not
//! changed by the reading, and a score is edited as a score. What an edit on
//! that page means to the sequence is the rule here: **it changes in the
//! sequence only what it changed on the page.**
//!
//! The page is rendered as it was before the edit and as it is after, and the
//! two renders are compared, item by item. An item both render alike is a
//! note the edit did not touch, and its events in the sequence stay as they
//! were -- with the time they were played at, their level, their curves. An
//! item they render differently takes the difference and nothing else: a key
//! the edit changed is written with its family's coherence, a move in time is
//! added to where the event was, a key the edit took away goes. An item only
//! the page after has is new events, and one only the page before had is
//! events removed. So moving one note's pitch on the page of a take leaves
//! every other note of the take where it was played; and what the edit wrote
//! is in the event's notation keys from then on, so the page reads it back.
//!
//! The same holds for what is no note's. A curve the two renders write alike
//! is left as the sequence has it -- drawn by hand or not -- and one they
//! write differently is the render's; what the page says of itself (its
//! meter, its key, its staves, what has two ends) is written into the
//! sequence's `notation` section, part by part, where the edit changed it.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use clausters_core::event::render as keys_of;
use clausters_core::notation::{Interpretation, Sheet};

use super::score::render;
use super::{CurveKind, Event, EventSequence, MidiSpec};
use crate::Opaque;
use crate::multitrack::Automation;

/// A sequence with a page's edit written back: the sequence, and which item
/// of the page after each of its events is.
#[derive(Debug, Clone, PartialEq)]
pub struct Written {
    /// The sequence, edited.
    pub sequence: EventSequence,
    /// `[event id, item id]`, as a reading answers it.
    pub items: Vec<(u64, u64)>,
}

/// The events each item of a render sounds, in the order it sounds them.
fn by_item(rendered: &EventSequence) -> BTreeMap<u64, Vec<&Event>> {
    let mut out: BTreeMap<u64, Vec<&Event>> = BTreeMap::new();
    let pairs = rendered
        .notation
        .as_ref()
        .and_then(|notation| notation.get("items"))
        .and_then(Value::as_array);
    for pair in pairs.into_iter().flatten() {
        let (Some(event), Some(item)) = (
            pair.get(0).and_then(Value::as_u64),
            pair.get(1).and_then(Value::as_u64),
        ) else {
            continue;
        };
        if let Some(event) = rendered.get(event) {
            out.entry(item).or_default().push(event);
        }
    }
    out
}

/// A render's event id as the item it sounds.
fn item_of(rendered: &EventSequence) -> BTreeMap<u64, u64> {
    by_item(rendered)
        .into_iter()
        .flat_map(|(item, events)| events.into_iter().map(move |event| (event.id, item)))
        .collect()
}

/// A curve without the identity a holder gave it.
fn bare(curve: &Automation) -> Automation {
    Automation {
        id: crate::NodeId(0),
        ..curve.clone()
    }
}

/// Whether two events of a render say the same thing in the same place.
fn alike(a: &Event, b: &Event) -> bool {
    a.at == b.at
        && a.data == b.data
        && a.automation.len() == b.automation.len()
        && a.automation
            .iter()
            .zip(&b.automation)
            .all(|(a, b)| bare(a) == bare(b))
}

/// What a curve of a sequence drives and on which channel: what makes two of
/// them the same lane, whatever either is called.
fn lane_of(curve: &Automation) -> String {
    let target = &curve.target.0;
    let kind = match CurveKind::of(target) {
        CurveKind::Cc(number) => format!("cc {number}"),
        CurveKind::Bend => "bend".into(),
        CurveKind::Pressure => "pressure".into(),
        CurveKind::Timbre => "timbre".into(),
        CurveKind::Control => format!("control {}", target.get("control").unwrap_or(&Value::Null)),
    };
    let channel = target
        .get("channel")
        .and_then(Value::as_f64)
        .map(|c| c as i64);
    format!("{kind} {channel:?}")
}

/// The MIDI note an event sounds, rounded.
fn midi_of(event: &Event) -> i64 {
    let keys = event.keys();
    keys_of::pitch_of(&keys)
        .midinote(&keys_of::scale_of(&keys))
        .round() as i64
}

/// How much a specification can say of one note: the order they widen in.
fn rank(spec: Option<MidiSpec>) -> u8 {
    match spec {
        None => 0,
        Some(MidiSpec::Midi1) => 1,
        Some(MidiSpec::Mpe { .. }) => 2,
        Some(MidiSpec::Midi2) => 3,
    }
}

/// **Write the edit that made `before` into `after` back to `sequence`** (see
/// the module). `items` is which item of `before` each event of the sequence
/// is, as the reading answered it, and `interp` the reading both pages are
/// rendered with.
///
/// # Errors
/// When either page cannot be rendered.
pub fn write_back(
    sequence: &EventSequence,
    items: &[(u64, u64)],
    before: &Sheet,
    after: &Sheet,
    interp: &Interpretation,
) -> Result<Written, String> {
    let (was, is) = (render(before, interp)?, render(after, interp)?);
    let (was_items, is_items) = (by_item(&was), by_item(&is));
    let mut out = sequence.clone();

    // The sequence's events of each item of the page before.
    let mut held: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for (event, item) in items {
        if out.get(*event).is_some() {
            held.entry(*item).or_default().push(*event);
        }
    }
    // What plays a new note: what plays the others of the sequence.
    let instrument = out
        .events
        .iter()
        .find_map(|event| event.keys().get("instrument").cloned());
    let mut now: Vec<(u64, u64)> = Vec::new();
    let mut gone: BTreeSet<u64> = BTreeSet::new();
    let mut added: Vec<(u64, Event)> = Vec::new();
    let fresh = |event: &Event, instrument: &Option<Value>| {
        let mut keys = event.keys();
        if let Some(instrument) = instrument {
            keys.entry("instrument")
                .or_insert_with(|| instrument.clone());
        }
        Event {
            id: 0,
            at: event.at,
            data: Opaque(Value::Object(keys)),
            automation: event.automation.iter().map(bare).collect(),
        }
    };
    for (item, sounded) in &is_items {
        let mine = held.get(item).cloned().unwrap_or_default();
        let Some(before) = was_items.get(item) else {
            // only the page after has it: new events
            added.extend(
                sounded
                    .iter()
                    .map(|event| (*item, fresh(event, &instrument))),
            );
            continue;
        };
        let same =
            before.len() == sounded.len() && before.iter().zip(sounded).all(|(a, b)| alike(a, b));
        if same {
            now.extend(mine.iter().map(|event| (*event, *item)));
            continue;
        }
        if before.len() != sounded.len() || mine.len() != before.len() {
            // it sounds another count of notes: its events are the render's
            gone.extend(mine);
            added.extend(
                sounded
                    .iter()
                    .map(|event| (*item, fresh(event, &instrument))),
            );
            continue;
        }
        // each event of the sequence with the note of the page it is: by
        // what it sounds, else in order
        let mut free: Vec<usize> = (0..before.len()).collect();
        for id in &mine {
            let Some(at) = out.events.iter().position(|event| event.id == *id) else {
                continue;
            };
            let sounds = midi_of(&out.events[at]);
            let pick = free
                .iter()
                .position(|i| midi_of(before[*i]) == sounds)
                .unwrap_or(0);
            let i = free.remove(pick);
            change(&mut out.events[at], before[i], sounded[i]);
            now.push((*id, *item));
        }
    }
    for (item, mine) in &held {
        if !is_items.contains_key(item) {
            gone.extend(mine.iter().copied());
        }
    }
    out.events.retain(|event| !gone.contains(&event.id));
    for (item, mut event) in added {
        out.next_id += 1;
        event.id = out.next_id;
        now.push((event.id, item));
        out.events.push(event);
    }

    // The curves: one the two renders write alike is the sequence's own.
    let lanes = |rendered: &EventSequence| -> BTreeMap<String, Automation> {
        rendered
            .automation
            .iter()
            .map(|curve| (lane_of(curve), bare(curve)))
            .collect()
    };
    let (was_lanes, is_lanes) = (lanes(&was), lanes(&is));
    let touched: BTreeSet<&String> = was_lanes
        .keys()
        .chain(is_lanes.keys())
        .filter(|lane| was_lanes.get(*lane) != is_lanes.get(*lane))
        .collect();
    out.automation
        .retain(|curve| !touched.contains(&lane_of(curve)));
    for lane in touched {
        if let Some(curve) = is_lanes.get(lane) {
            out.automation.push(curve.clone());
        }
    }
    if was.tempo_map != is.tempo_map {
        out.tempo_map = is.tempo_map.clone();
    }

    // What the page says of itself, part by part.
    let first: BTreeMap<u64, u64> = now
        .iter()
        .rev()
        .map(|(event, item)| (*item, *event))
        .collect();
    let (was_said, is_said) = (section(&was), section(&is));
    let mut said = out
        .notation
        .as_ref()
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for part in ["grid", "key", "staves", "header", "page", "groups"] {
        if was_said.get(part) != is_said.get(part) {
            match is_said.get(part) {
                Some(value) => said.insert(part.into(), value.clone()),
                None => said.remove(part),
            };
        }
    }
    let (was_item, is_item) = (item_of(&was), item_of(&is));
    for (part, ends) in [
        ("spanners", &["from", "to"][..]),
        ("controls", &["on"][..]),
        ("marks", &[][..]),
    ] {
        // by item, which is what the two pages share
        let named = |said: &Map<String, Value>, items: &BTreeMap<u64, u64>| {
            translated(said.get(part), ends, &|event| items.get(&event).copied())
        };
        let (a, b) = (named(&was_said, &was_item), named(&is_said, &is_item));
        if a != b {
            let events = translated(Some(&Value::Array(b)), ends, &|item| {
                first.get(&item).copied()
            });
            said.insert(part.into(), Value::Array(events));
        }
    }
    if said.contains_key("items") {
        now.sort();
        said.insert("items".into(), json!(now));
    }
    if !said.is_empty() {
        out.notation = Some(Value::Object(said));
    }

    out.hold();
    // a note's curve the page wrote may be more than the sequence's
    // specification says
    if out.midi.is_some() {
        let fit = out.midi_fit();
        if rank(fit) > rank(out.midi) {
            out.midi = fit;
        }
    }
    now.sort();
    Ok(Written {
        sequence: out,
        items: now,
    })
}

/// A render's `notation` section.
fn section(rendered: &EventSequence) -> Map<String, Value> {
    rendered
        .notation
        .as_ref()
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// A list of the section whose entries name events -- under `ends`, or as the
/// first of a pair where there are none -- with each name put through `name`;
/// an entry one of whose names has no answer is left out.
fn translated(
    list: Option<&Value>,
    ends: &[&str],
    name: &dyn Fn(u64) -> Option<u64>,
) -> Vec<Value> {
    list.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let mut entry = entry.clone();
            if ends.is_empty() {
                let named = name(entry.get(0)?.as_u64()?)?;
                *entry.get_mut(0)? = json!(named);
            } else {
                for end in ends {
                    let named = name(entry.get(*end)?.as_u64()?)?;
                    entry[*end] = json!(named);
                }
            }
            Some(entry)
        })
        .collect()
}

/// **What an edit did to one note, done to the event that is it**: the keys
/// the two renders differ in, written with their families' coherence -- the
/// written pitch last, so it is the one that stands -- a move in time added
/// to where the event is, and its curves where they differ.
fn change(event: &mut Event, before: &Event, after: &Event) {
    let (was, is) = (before.keys(), after.keys());
    let mut keys = event.keys();
    for key in was.keys() {
        if !is.contains_key(key) {
            keys.remove(key);
        }
    }
    let mut differing: Vec<&String> = is
        .keys()
        .filter(|key| was.get(*key) != is.get(*key))
        .collect();
    differing.sort_by_key(|key| key.as_str() == "pitches");
    for key in differing {
        keys_of::set_key(&mut keys, key, is[key].clone());
    }
    event.data = Opaque(Value::Object(keys));
    event.at.0 += after.at.0 - before.at.0;
    let curves = |event: &Event| event.automation.iter().map(bare).collect::<Vec<_>>();
    if curves(before) != curves(after) {
        event.automation = curves(after);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::transcription::{Transcription, read};
    use clausters_core::notation::{Item, Marks, Pitch, Spanner, Step};
    use clausters_core::ratio::Ratio;

    /// A take played a little off the grid, with a curve drawn over it.
    fn take() -> EventSequence {
        serde_json::from_value(json!({
            "events": [
                {"at": 0.03, "data": {"midinote": 60, "dur": 0.93, "velocity": 71, "instrument": "piano"}},
                {"at": 1.01, "data": {"midinote": 62, "dur": 0.97, "velocity": 64, "instrument": "piano"},
                 "automation": [{"id": 0, "target": {"bend": true},
                                 "points": [{"at": 0.0, "value": 0.0}, {"at": 0.4, "value": 0.3}]}]},
                {"at": 1.98, "data": {"midinote": 64, "dur": 1.02, "velocity": 80, "instrument": "piano"}},
                {"at": 3.02, "data": {"midinote": 65, "dur": 0.9, "velocity": 77, "instrument": "piano"}},
            ],
            "automation": [{"id": 0, "target": {"cc": 1, "channel": 0},
                            "points": [{"at": 0.0, "value": 10.0}, {"at": 2.0, "value": 90.0}]}],
        }))
        .unwrap()
    }

    fn page(seq: &EventSequence) -> (Sheet, Vec<(u64, u64)>) {
        let how = Transcription {
            dynamics: false,
            ..Transcription::default()
        };
        let got = read(seq, &how, &Interpretation::default()).unwrap();
        (got.sheet, got.items)
    }

    fn written(seq: &EventSequence, edit: impl FnOnce(&mut Sheet)) -> Written {
        let (before, items) = page(seq);
        let mut after = before.clone();
        edit(&mut after);
        write_back(seq, &items, &before, &after, &Interpretation::default()).unwrap()
    }

    fn pitch(step: Step, alter: i32) -> Pitch {
        Pitch {
            step,
            alter,
            octave: 4,
            forced: false,
        }
    }

    #[test]
    fn a_note_moved_on_the_page_moves_alone() {
        let seq = take();
        let got = written(&seq, |sheet| {
            if let Item::Note { pitches, .. } = &mut sheet.staves[0].voices[0].items[2] {
                pitches[0] = pitch(Step::F, 1);
            }
        });
        // the third note is an F sharp now, written as one
        let third = &got.sequence.events[2];
        assert_eq!(third.keys()["midinote"], json!(66.0));
        assert_eq!(third.keys()["pitches"][0]["step"], "f");
        // played when it was, as loud as it was, by what played it
        assert_eq!(third.at.0, 1.98);
        assert_eq!(third.keys()["velocity"], json!(80));
        assert_eq!(third.keys()["instrument"], "piano");
        // and nothing else of the take moved: the others, the bend, the curve
        for i in [0, 1, 3] {
            assert_eq!(got.sequence.events[i], seq.events[i]);
        }
        assert_eq!(got.sequence.automation, seq.automation);
        assert_eq!(
            got.sequence.notation, None,
            "the page said nothing new of itself"
        );
        assert_eq!(got.items.len(), 4);
        // read again, the page shows the F sharp
        let (again, _) = page(&got.sequence);
        assert_eq!(
            again.staves[0].voices[0].items[2].pitches()[0],
            pitch(Step::F, 1)
        );
    }

    #[test]
    fn a_note_written_in_shifts_what_follows_by_its_length() {
        let seq = take();
        let got = written(&seq, |sheet| {
            sheet.staves[0].voices[0].items.insert(
                1,
                Item::Note {
                    id: 99,
                    pitches: vec![pitch(Step::G, 0)],
                    dur: Ratio::new(1, 4),
                    tie: false,
                    marks: Marks::default(),
                },
            );
        });
        assert_eq!(got.sequence.events.len(), 5);
        let new = got
            .sequence
            .events
            .iter()
            .find(|event| event.keys()["midinote"] == json!(67))
            .expect("the new note");
        assert_eq!(new.at.0, 1.0);
        assert_eq!(
            new.keys()["instrument"],
            "piano",
            "played by what plays the take"
        );
        // the notes after it are a beat later, each as far off the beat as
        // it was played
        let at = |midi: i64| {
            got.sequence
                .events
                .iter()
                .find(|event| event.keys()["midinote"] == json!(midi))
                .map(|event| event.at.0)
                .unwrap()
        };
        assert!((at(62) - 2.01).abs() < 1e-9);
        assert!((at(65) - 4.02).abs() < 1e-9);
        assert_eq!(at(60), 0.03);
    }

    #[test]
    fn a_note_taken_out_and_a_slur_written_reach_the_sequence() {
        let seq = take();
        let got = written(&seq, |sheet| {
            let items = &mut sheet.staves[0].voices[0].items;
            let (first, second) = (items[0].id(), items[1].id());
            items[3] = items[3].silenced();
            sheet.spanners.push(Spanner {
                kind: "slur".into(),
                from: first,
                to: second,
            });
        });
        assert_eq!(got.sequence.events.len(), 3);
        // what has two ends names the sequence's own events
        let ids: Vec<u64> = got.sequence.events.iter().map(|event| event.id).collect();
        assert_eq!(
            got.sequence.notation.as_ref().unwrap()["spanners"],
            json!([{"kind": "slur", "from": ids[0], "to": ids[1]}])
        );
        // and the page read again has the slur
        let (again, _) = page(&got.sequence);
        assert_eq!(again.spanners.len(), 1);
        assert_eq!(again.staves[0].voices[0].items.len(), 3);
    }

    #[test]
    fn a_dynamic_written_changes_the_levels_and_the_lane_and_no_time() {
        let seq = take();
        let got = written(&seq, |sheet| {
            if let Item::Note { marks, .. } = &mut sheet.staves[0].voices[0].items[2] {
                marks.dynamic = Some("ff".into());
            }
        });
        // the levels from the mark on are the mark's; the notes before it
        // keep theirs
        assert_eq!(got.sequence.events[0], seq.events[0]);
        assert_eq!(got.sequence.events[2].keys()["dynamic"], "ff");
        assert!(got.sequence.events[2].keys()["amp"].as_f64().unwrap() > 0.2);
        assert_eq!(got.sequence.events[2].at.0, 1.98);
        assert_eq!(got.sequence.events[3].at.0, 3.02);
        // the dynamics' lane is the page's, and the curve drawn by hand stays
        assert!(
            got.sequence
                .automation
                .iter()
                .any(|c| c.target.0["cc"] == 11)
        );
        assert!(
            got.sequence
                .automation
                .iter()
                .any(|c| c.target.0["cc"] == 1)
        );
    }
}
