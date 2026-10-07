//! **A sequence as plain data, and a sequence in parts.**
//!
//! A sequence holds everything in its events, and a caller often wants it
//! back as numbers of its own: `[(note, dur), ...]`, or any other set of
//! keys. [`rows`] is that projection -- the caller chooses the keys, and gets
//! one row per event, each a list of values in the keys' order -- and
//! [`from_rows`] is the way back, so a list of plain numbers is a sequence a
//! roll and a page can open.
//!
//! # How a key is read
//!
//! As the event means it, not as it happens to spell it. A note written by
//! its `degree`, its `freq` or only its `pitches` still has a `midinote`, and
//! one that states a `velocity` still has an `amp`: the pitch and the level
//! are families (`clausters_core::event`) and a row reads through them, as
//! `sustain` is read through `dur` and `legato`. `at` and `id` are the
//! event's own place and identity. Any other key is the value the event
//! holds, and a key it does not hold is null.
//!
//! # A line
//!
//! One row per event is the sequence as it is placed, and says its time only
//! where `at` is among the keys. [`line()`] is the other shape a caller asks
//! for: the rows of **one line played back to back**, the way a pattern
//! writes one. Notes that start together are one row, a key they differ in
//! holding the list of their values (a chord); `dur` is the time to the next
//! row; and a silence -- where a row's own length ends before the next one
//! starts -- is a row of its own, every key null but its `dur`.
//!
//! # Parts
//!
//! A score rendered into a sequence is all its voices, a channel each. A
//! voice, a staff or a channel is a line a caller may want on its own -- as
//! its own rows, on its own track -- and [`separate`] makes each a sequence,
//! with the curves of its channels and the MIDI specification that says what
//! it holds ([`EventSequence::midi_fit`]).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use clausters_core::event::render::{self, Type};

use super::{CurveKind, Event, EventSequence, MidiSpec};

/// How near two beats are to be the same onset.
const SAME: f64 = 1e-9;

/// The keys that are a list by nature: one of them holding a list is one
/// value, not a chord.
const LISTS: [&str; 4] = ["pitches", "value", "articulations", "scale"];

/// The pitch keys: a row that states none of the ones it was asked for is a
/// silence.
const PITCHES: [&str; 4] = ["midinote", "freq", "degree", "pitches"];

/// The most notes an MPE zone sounds at once: its member channels.
const MPE_MEMBERS: usize = 15;

/// The most channels a MIDI port has.
const MIDI_CHANNELS: usize = 16;

/// **One key of `event`, as the event means it** (see the module).
#[must_use]
pub fn read(event: &Event, key: &str) -> Value {
    let keys = event.keys();
    let held = || keys.get(key).cloned().unwrap_or(Value::Null);
    match key {
        "at" => return json!(event.at.0),
        "id" => return json!(event.id),
        "dur" => return json!(length_of(&keys).0),
        _ => {}
    }
    if Type::of(&keys) != Type::Note {
        return held();
    }
    match key {
        "midinote" => json!(render::pitch_of(&keys).midinote(&render::scale_of(&keys))),
        "freq" => json!(render::pitch_of(&keys).freq(&render::scale_of(&keys))),
        "amp" => json!(render::level_of(&keys).amp()),
        "velocity" => json!(render::level_of(&keys).velocity()),
        "sustain" => json!(render::sustain_of(&keys)),
        _ => held(),
    }
}

/// An event's written length in beats -- its `dur`, a beat where it states
/// none -- and how long it sounds.
fn length_of(keys: &Map<String, Value>) -> (f64, f64) {
    let dur = keys.get("dur").and_then(Value::as_f64).unwrap_or(1.0);
    (dur, render::sustain_of(keys))
}

/// **The sequence as rows**: one per event, in the events' order, each the
/// values of `keys` in that order.
#[must_use]
pub fn rows(sequence: &EventSequence, keys: &[String]) -> Vec<Vec<Value>> {
    sequence
        .events
        .iter()
        .map(|event| keys.iter().map(|key| read(event, key)).collect())
        .collect()
}

/// **The sequence as one line** (see the module): its notes, the ones that
/// start together as one row, each row lasting to the next, and a silence a
/// row of its own.
#[must_use]
pub fn line(sequence: &EventSequence, keys: &[String]) -> Vec<Vec<Value>> {
    let notes: Vec<&Event> = sequence
        .events
        .iter()
        .filter(|event| Type::of(&event.keys()) == Type::Note)
        .collect();
    let mut groups: Vec<Vec<&Event>> = Vec::new();
    for note in notes {
        match groups.last_mut() {
            Some(group) if (group[0].at.0 - note.at.0).abs() <= SAME => group.push(note),
            _ => groups.push(vec![note]),
        }
    }
    let silence = |at: f64, dur: f64| -> Vec<Value> {
        keys.iter()
            .map(|key| match key.as_str() {
                "at" => json!(at),
                "dur" => json!(dur),
                "type" => json!("rest"),
                _ => Value::Null,
            })
            .collect()
    };
    let mut out = Vec::new();
    if let Some(first) = groups.first()
        && first[0].at.0 > SAME
    {
        out.push(silence(0.0, first[0].at.0));
    }
    for (i, group) in groups.iter().enumerate() {
        let at = group[0].at.0;
        // The row's own length is its longest note's; it lasts no further
        // than the next row starts.
        let own = group
            .iter()
            .map(|event| length_of(&event.keys()).0)
            .fold(0.0, f64::max);
        let next = groups.get(i + 1).map(|g| g[0].at.0);
        let dur = next.map_or(own, |next| own.min(next - at));
        out.push(
            keys.iter()
                .map(|key| match key.as_str() {
                    "at" => json!(at),
                    "dur" => json!(dur),
                    _ => together(group, key),
                })
                .collect(),
        );
        if let Some(next) = next
            && next - (at + dur) > SAME
        {
            out.push(silence(at + dur, next - (at + dur)));
        }
    }
    out
}

/// What the notes of one row say for `key`: the value they share, or the
/// list of theirs. The written pitches of a chord are one list, a pitch to a
/// note; any other key that is a list by nature is the first note's.
fn together(group: &[&Event], key: &str) -> Value {
    let values: Vec<Value> = group.iter().map(|event| read(event, key)).collect();
    if key == "pitches" {
        let all: Vec<Value> = values
            .iter()
            .filter_map(Value::as_array)
            .flatten()
            .cloned()
            .collect();
        return if all.is_empty() {
            Value::Null
        } else {
            Value::Array(all)
        };
    }
    if values.windows(2).all(|pair| pair[0] == pair[1]) || LISTS.contains(&key) {
        return values.into_iter().next().unwrap_or(Value::Null);
    }
    Value::Array(values)
}

/// **A sequence from rows** of `keys`: the way back from [`rows`] and from
/// [`line()`].
///
/// Where `at` is among the keys each row is placed there; otherwise the rows
/// play back to back, each lasting its `dur` (a beat where there is none). A
/// row that states none of the pitch keys it was asked for is a silence: it
/// takes its time and is no event. A key holding a list is a chord, a note to
/// each value -- and `pitches`, a note to each written pitch. A null is a key
/// the event does not state; `id` is not read, since a sequence mints its
/// own.
///
/// # Errors
/// When a row is not a list as long as `keys`.
pub fn from_rows(keys: &[String], rows: &[Value]) -> Result<EventSequence, String> {
    let placed = keys.iter().any(|key| key == "at");
    let asked: Vec<&String> = keys
        .iter()
        .filter(|key| PITCHES.contains(&key.as_str()))
        .collect();
    let mut cursor = 0.0;
    let mut events = Vec::new();
    for (n, row) in rows.iter().enumerate() {
        let row = row
            .as_array()
            .filter(|row| row.len() == keys.len())
            .ok_or_else(|| format!("row {n} is not a list of {} values", keys.len()))?;
        let of = |key: &str| {
            keys.iter()
                .position(|k| k == key)
                .map(|i| &row[i])
                .filter(|value| !value.is_null())
        };
        let at = if placed {
            of("at").and_then(Value::as_f64).unwrap_or(cursor)
        } else {
            cursor
        };
        let dur = of("dur").and_then(Value::as_f64).unwrap_or(1.0);
        cursor = at + dur;
        let silent = of("type").and_then(Value::as_str) == Some("rest")
            || (!asked.is_empty() && asked.iter().all(|key| of(key).is_none()));
        if silent {
            continue;
        }
        // How many notes the row is: the longest of its chord keys.
        let voices = keys
            .iter()
            .zip(row)
            .filter_map(|(key, value)| {
                let list = value.as_array()?;
                (key == "pitches" || !LISTS.contains(&key.as_str())).then_some(list.len())
            })
            .max()
            .unwrap_or(1)
            .max(1);
        for voice in 0..voices {
            let mut data = Map::new();
            for (key, value) in keys.iter().zip(row) {
                if value.is_null() || key == "at" || key == "id" {
                    continue;
                }
                let one = match value.as_array() {
                    Some(list) if key == "pitches" => match list.get(voice.min(list.len() - 1)) {
                        Some(pitch) => json!([pitch]),
                        None => continue,
                    },
                    Some(list) if !LISTS.contains(&key.as_str()) => {
                        match list.get(voice.min(list.len().saturating_sub(1))) {
                            Some(value) => value.clone(),
                            None => continue,
                        }
                    }
                    _ => value.clone(),
                };
                data.insert(key.clone(), one);
            }
            events.push(Event::new(at, Value::Object(data)));
        }
    }
    Ok(EventSequence::new(events))
}

/// What a sequence is separated by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum By {
    /// Each voice of each staff: the events' `staff` and `voice`.
    Voice,
    /// Each staff: the events' `staff`.
    Staff,
    /// Each channel: the events' `channel`.
    Channel,
}

impl By {
    /// The separation a word names (`"voice"`, `"staff"`, `"channel"`).
    #[must_use]
    pub fn parse(word: &str) -> Option<By> {
        match word {
            "voice" => Some(By::Voice),
            "staff" => Some(By::Staff),
            "channel" => Some(By::Channel),
            _ => None,
        }
    }

    /// Where an event with these keys goes: `(staff, voice)`, the staff
    /// alone, or the channel -- zero for what it does not state.
    fn place(self, keys: &Map<String, Value>) -> (i64, i64) {
        let number = |key: &str| keys.get(key).and_then(Value::as_f64).unwrap_or(0.0) as i64;
        match self {
            By::Voice => (number("staff"), number("voice")),
            By::Staff => (number("staff"), 0),
            By::Channel => (number("channel"), 0),
        }
    }
}

/// An event's channel, 0 where it names none.
fn channel_of(keys: &Map<String, Value>) -> i64 {
    keys.get("channel").and_then(Value::as_f64).unwrap_or(0.0) as i64
}

/// **The sequence in parts** (see the module), in the order of what they are
/// separated by, a part for each voice, staff or channel that has events.
///
/// Each part is a sequence of its own: the events of its line with the ids
/// they had, the curves of the channels those events are on (a curve that
/// names no channel goes to every part), the tempo map, and the MIDI
/// specification that says what it holds where the whole named one. What the
/// whole says of its page goes with it too, cut to the part: by voice or by
/// staff the part is a score of its own, so its staff is the top one and its
/// events say so, and a spanner is kept where both its ends are.
#[must_use]
pub fn separate(sequence: &EventSequence, by: By) -> Vec<EventSequence> {
    let mut placed: BTreeMap<(i64, i64), Vec<Event>> = BTreeMap::new();
    for event in &sequence.events {
        placed
            .entry(by.place(&event.keys()))
            .or_default()
            .push(event.clone());
    }
    placed
        .into_iter()
        .map(|(place, mut events)| {
            let channels: BTreeSet<i64> = events
                .iter()
                .map(|event| channel_of(&event.keys()))
                .collect();
            let ids: BTreeSet<u64> = events.iter().map(|event| event.id).collect();
            if by != By::Channel {
                for event in &mut events {
                    if let Some(keys) = event.data.0.as_object_mut() {
                        if keys.contains_key("staff") {
                            keys.insert("staff".into(), json!(0));
                        }
                        if by == By::Voice && keys.contains_key("voice") {
                            keys.insert("voice".into(), json!(0));
                        }
                    }
                }
            }
            let automation = sequence
                .automation
                .iter()
                .filter(|curve| {
                    curve
                        .target
                        .0
                        .get("channel")
                        .and_then(Value::as_f64)
                        .is_none_or(|channel| channels.contains(&(channel as i64)))
                })
                .cloned()
                .collect();
            let mut part = EventSequence {
                events,
                automation,
                tempo_map: sequence.tempo_map.clone(),
                notation: sequence
                    .notation
                    .as_ref()
                    .map(|notation| cut(notation, by, place, &ids)),
                next_id: sequence.next_id,
                extra: sequence.extra.clone(),
                midi: None,
            };
            part.midi = sequence.midi.and_then(|_| part.midi_fit());
            part
        })
        .collect()
}

/// A sequence's `notation` section cut to one part: its staff alone where it
/// is separated by voice or by staff, and the spanners and the items whose
/// events it holds.
fn cut(notation: &Value, by: By, place: (i64, i64), ids: &BTreeSet<u64>) -> Value {
    let mut out = notation.clone();
    let Some(map) = out.as_object_mut() else {
        return out;
    };
    let held = |value: Option<&Value>| {
        value
            .and_then(Value::as_u64)
            .is_some_and(|id| ids.contains(&id))
    };
    if by != By::Channel
        && let Some(staff) = map
            .get("staves")
            .and_then(Value::as_array)
            .and_then(|staves| staves.get(place.0 as usize))
            .cloned()
    {
        let mut staff = staff;
        if by == By::Voice
            && let Some(staff) = staff.as_object_mut()
        {
            staff.insert("voices".into(), json!(1));
        }
        map.insert("staves".into(), json!([staff]));
    }
    if let Some(Value::Array(spanners)) = map.get_mut("spanners") {
        spanners.retain(|spanner| held(spanner.get("from")) && held(spanner.get("to")));
    }
    if let Some(Value::Array(items)) = map.get_mut("items") {
        items.retain(|pair| held(pair.get(0)));
    }
    out
}

impl EventSequence {
    /// **The MIDI specification that says what this sequence holds**, the
    /// narrowest one, or `None` where none does.
    ///
    /// A curve is a channel's or one note's, whatever the specification, and
    /// the specification is what says the second: MIDI 1.0 says no curve of
    /// one note but its pressure. So a sequence whose notes carry no curve
    /// of their own is **MIDI 1.0**, a channel to a line. One whose notes do
    /// is **MPE** where it is one line -- one channel, and no more notes at
    /// once than a zone has members -- since there each note takes a channel
    /// and the line's own curves the master's; and **MIDI 2.0** where it is
    /// several, which says a note's curve and a channel's side by side. A
    /// curve over a def's control has no MIDI spelling and decides nothing,
    /// and neither does a note's pressure, which every one of them says.
    #[must_use]
    pub fn midi_fit(&self) -> Option<MidiSpec> {
        let mut channels: BTreeSet<i64> = self
            .events
            .iter()
            .map(|event| channel_of(&event.keys()))
            .collect();
        channels.extend(self.automation.iter().filter_map(|curve| {
            curve
                .target
                .0
                .get("channel")
                .and_then(Value::as_f64)
                .map(|channel| channel as i64)
        }));
        let kinds: Vec<CurveKind> = self
            .events
            .iter()
            .flat_map(|event| event.automation.iter())
            .map(|curve| CurveKind::of(&curve.target.0))
            .filter(|kind| !matches!(kind, CurveKind::Control | CurveKind::Pressure))
            .collect();
        if kinds.is_empty() {
            return (channels.len() <= MIDI_CHANNELS).then_some(MidiSpec::Midi1);
        }
        // A zone's member says a note's bend, pressure and timbre, and no
        // controller of its own.
        let zone = channels.len() <= 1
            && self.most_at_once() <= MPE_MEMBERS
            && !kinds.iter().any(|kind| matches!(kind, CurveKind::Cc(_)));
        Some(if zone {
            MidiSpec::Mpe {
                upper: false,
                members: MPE_MEMBERS as u8,
            }
        } else {
            MidiSpec::Midi2
        })
    }

    /// The most notes sounding at one time.
    fn most_at_once(&self) -> usize {
        let mut edges: Vec<(f64, i32)> = self
            .events
            .iter()
            .filter(|event| Type::of(&event.keys()) == Type::Note)
            .flat_map(|event| {
                let end = event.at.0 + event.sustain().max(0.0);
                [(event.at.0, 1), (end, -1)]
            })
            .collect();
        // a note that ends where another starts has ended
        edges.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let (mut now, mut most) = (0i32, 0i32);
        for (_, edge) in edges {
            now += edge;
            most = most.max(now);
        }
        most.max(0) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    fn sequence(events: Value) -> EventSequence {
        serde_json::from_value(json!({ "events": events })).expect("a sequence")
    }

    #[test]
    fn a_row_reads_each_key_as_the_event_means_it() {
        let seq = sequence(json!([
            {"at": 0.0, "data": {"midinote": 60, "dur": 1.0}},
            {"at": 1.0, "data": {"degree": 2, "dur": 0.5, "velocity": 127}},
            {"at": 1.5, "data": {"pitches": [{"step": "e", "alter": -1, "octave": 4}], "dur": 2.0}},
        ]));
        assert_eq!(
            rows(&seq, &keys(&["midinote", "dur"])),
            vec![
                vec![json!(60.0), json!(1.0)],
                vec![json!(64.0), json!(0.5)],
                vec![json!(63.0), json!(2.0)],
            ]
        );
        let placed = rows(&seq, &keys(&["at", "amp", "sustain", "pan", "id"]));
        assert_eq!(placed[1][0], json!(1.0));
        assert_eq!(placed[1][1], json!(1.0), "a velocity has an amp");
        assert_eq!(
            placed[0][2],
            json!(0.8),
            "a sustain is read through its dur"
        );
        assert_eq!(placed[0][3], Value::Null, "a key it does not hold");
        assert_eq!(placed[2][4], json!(3));
    }

    #[test]
    fn a_line_plays_back_to_back_with_its_chords_and_its_silences() {
        let seq = sequence(json!([
            {"at": 1.0, "data": {"midinote": 60, "dur": 1.0}},
            {"at": 2.0, "data": {"midinote": 64, "dur": 1.0, "amp": 0.5}},
            {"at": 2.0, "data": {"midinote": 67, "dur": 1.0, "amp": 0.5}},
            {"at": 4.0, "data": {"midinote": 72, "dur": 2.0}},
            {"at": 4.5, "data": {"midinote": 74, "dur": 0.5}},
        ]));
        let read = line(&seq, &keys(&["midinote", "dur", "amp"]));
        assert_eq!(
            read,
            vec![
                vec![Value::Null, json!(1.0), Value::Null],
                vec![json!(60.0), json!(1.0), json!(0.1)],
                vec![json!([64.0, 67.0]), json!(1.0), json!(0.5)],
                vec![Value::Null, json!(1.0), Value::Null],
                // a note the next one starts under lasts to that start
                vec![json!(72.0), json!(0.5), json!(0.1)],
                vec![json!(74.0), json!(0.5), json!(0.1)],
            ]
        );
    }

    #[test]
    fn rows_read_back_are_the_line_they_were_read_from() {
        let wanted = keys(&["midinote", "dur"]);
        let written = json!([
            [null, 0.5],
            [60.0, 1.0],
            [[64.0, 67.0], 1.5],
            [null, 1.0],
            [72.0, 2.0]
        ]);
        let seq = from_rows(&wanted, written.as_array().unwrap()).expect("rows");
        assert_eq!(
            seq.events.iter().map(|e| e.at.0).collect::<Vec<_>>(),
            [0.5, 1.5, 1.5, 4.0]
        );
        assert_eq!(json!(line(&seq, &wanted)), written);
        // placed rows keep their places, and a chord of written pitches is a
        // note to each
        let placed = from_rows(
            &keys(&["at", "pitches", "value"]),
            json!([[2.0, [{"step": "c", "octave": 4}, {"step": "g", "octave": 4}], [1, 4]]])
                .as_array()
                .unwrap(),
        )
        .expect("rows");
        assert_eq!(placed.events.len(), 2);
        assert_eq!(placed.events[1].at.0, 2.0);
        assert_eq!(read(&placed.events[1], "midinote"), json!(67.0));
        assert_eq!(read(&placed.events[1], "value"), json!([1, 4]));
        assert!(from_rows(&wanted, &[json!([60.0])]).is_err());
    }

    /// Two staves, the top one in two voices, a channel to each voice.
    fn rendered() -> EventSequence {
        serde_json::from_value(json!({
            "events": [
                {"id": 1, "at": 0.0, "data": {"midinote": 72, "dur": 1.0, "channel": 0, "staff": 0, "voice": 0}},
                {"id": 2, "at": 1.0, "data": {"midinote": 74, "dur": 1.0, "channel": 0, "staff": 0, "voice": 0}},
                {"id": 3, "at": 0.0, "data": {"midinote": 60, "dur": 2.0, "channel": 1, "staff": 0, "voice": 1}},
                {"id": 4, "at": 0.0, "data": {"midinote": 48, "dur": 2.0, "channel": 2, "staff": 1, "voice": 0}},
            ],
            "automation": [
                {"id": 5, "name": "dynamics", "target": {"cc": 11, "channel": 0, "group": "staff 1"},
                 "points": [{"at": 0.0, "value": 80.0}]},
                {"id": 6, "name": "dynamics", "target": {"cc": 11, "channel": 1, "group": "staff 1"},
                 "points": [{"at": 0.0, "value": 80.0}]},
                {"id": 7, "name": "dynamics", "target": {"cc": 11, "channel": 2},
                 "points": [{"at": 0.0, "value": 50.0}]},
            ],
            "midi": "1.0",
            "notation": {
                "key": "C",
                "staves": [{"clef": "G2", "voices": 2}, {"clef": "F4", "voices": 1}],
                "spanners": [{"kind": "slur", "from": 1, "to": 2}, {"kind": "slur", "from": 2, "to": 3}],
                "items": [[1, 11], [2, 12], [3, 13], [4, 14]],
            },
        }))
        .expect("a sequence")
    }

    #[test]
    fn a_sequence_is_separated_into_its_voices_each_with_its_curves() {
        let whole = rendered();
        let voices = separate(&whole, By::Voice);
        assert_eq!(voices.len(), 3);
        let top = &voices[0];
        assert_eq!(top.events.iter().map(|e| e.id).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(top.automation.len(), 1);
        assert_eq!(top.automation[0].id.0, 5);
        assert_eq!(top.midi, Some(MidiSpec::Midi1));
        let notation = top.notation.as_ref().unwrap();
        assert_eq!(notation["staves"], json!([{"clef": "G2", "voices": 1}]));
        assert_eq!(
            notation["spanners"],
            json!([{"kind": "slur", "from": 1, "to": 2}])
        );
        assert_eq!(notation["items"], json!([[1, 11], [2, 12]]));
        // the bass is a score of its own: its staff is the top one
        let bass = &voices[2];
        assert_eq!(read(&bass.events[0], "staff"), json!(0));
        assert_eq!(
            read(&bass.events[0], "channel"),
            json!(2),
            "its channel is kept"
        );
        assert_eq!(bass.notation.as_ref().unwrap()["staves"][0]["clef"], "F4");

        let staves = separate(&whole, By::Staff);
        assert_eq!(staves.len(), 2);
        assert_eq!(staves[0].events.len(), 3);
        assert_eq!(staves[0].automation.len(), 2, "both lanes of the group");
        assert_eq!(
            read(&staves[0].events[1], "voice"),
            json!(1),
            "its voices stay two"
        );
        assert_eq!(separate(&whole, By::Channel).len(), 3);
    }

    #[test]
    fn the_specification_follows_what_the_notes_carry() {
        let bend = json!([{"id": 0, "target": {"bend": true},
                           "points": [{"at": 0.0, "value": 0.0}, {"at": 1.0, "value": 2.0}]}]);
        // no curve of a note's own: a channel to a line
        assert_eq!(rendered().midi_fit(), Some(MidiSpec::Midi1));
        // one line whose notes bend: a channel to a note
        let mut chord = sequence(json!([
            {"at": 0.0, "data": {"midinote": 60, "dur": 1.0}, "automation": bend},
            {"at": 0.0, "data": {"midinote": 64, "dur": 1.0}},
        ]));
        assert!(matches!(
            chord.midi_fit(),
            Some(MidiSpec::Mpe {
                upper: false,
                members: 15
            })
        ));
        // a hairpin inside a note is over a def's control, which MIDI names
        // nothing for: it decides nothing
        let hairpin =
            json!([{"id": 0, "target": {"control": "amp"}, "points": [{"at": 0.0, "value": 0.1}]}]);
        let held = sequence(json!([{"at": 0.0, "data": {"midinote": 60}, "automation": hairpin}]));
        assert_eq!(held.midi_fit(), Some(MidiSpec::Midi1));
        // several lines, one of them bending: a note's curve beside a channel's
        chord
            .events
            .push(Event::new(0.0, json!({"midinote": 48, "channel": 1})));
        assert_eq!(chord.midi_fit(), Some(MidiSpec::Midi2));
        // and a part of it is what it holds: the bending line alone is a zone
        chord.midi = Some(MidiSpec::Midi2);
        let parts = separate(&EventSequence::new(chord.events.clone()), By::Channel);
        assert_eq!(
            parts[0].midi, None,
            "a sequence that named no specification names none"
        );
        chord.hold();
        let parts = separate(&chord, By::Channel);
        assert!(matches!(parts[0].midi, Some(MidiSpec::Mpe { .. })));
        assert_eq!(parts[1].midi, Some(MidiSpec::Midi1));
    }
}
