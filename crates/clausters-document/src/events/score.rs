//! **A score rendered into a sequence**, one way.
//!
//! A score's structure is its sheet (`clausters_core::notation::Sheet`):
//! exact values, voices over a grid of its own, spanners over item ids. A
//! sequence is what plays -- events in beats, with their curves -- and what a
//! roll edits. [`render`] makes the second from the first, as a timeline and a
//! pattern are rendered: nothing travels back, and what a roll then does to
//! the sequence stays in the sequence.
//!
//! What a render keeps:
//!
//! - **Every sounding note is an event**, a chord one event per note. Its
//!   sounding keys are the interpreter's (`perform`): `midinote`, the written
//!   length as `dur`, the held one as `sustain`, the level as `amp`. Beside
//!   them it carries what it is on the page, in the notation keys
//!   (`clausters_core::event::notation`): `pitches`, `value`, `staff`,
//!   `voice` and its marks -- so an event still says the F flat it was.
//! - **A voice is a channel.** Each voice of each staff renders on a channel
//!   of its own, counted from the top, and an event names its `channel`: what
//!   governs a line is then a lane of that channel.
//! - **A dynamic is a lane too** ([`levels`]), unless the reading hears it in
//!   the attack alone: the staff's level over time, on the controller the
//!   reading names. On a staff with one voice it is that channel's. On a
//!   staff with several it governs them all, and that is a **channel group**:
//!   the same lane on each channel of the group, each naming the `group` in
//!   its target -- so everything that plays a lane by its channel plays it
//!   with nothing new to learn, and what edits them as one finds them by that
//!   name.
//! - **What is no note's** goes whole into the sequence's `notation` section:
//!   the grid, the key, the clefs, the header, the page, and the spanners,
//!   whose two ends are named by event id. `items` there says which item of
//!   the sheet each event came from, since a chord's events share one.
//!
//! - **What the page does to the time** is the sequence's tempo map: the
//!   repeats, endings and jumps played out, a tempo mark the tempo from
//!   where it is heard.
//!   A note held under a hairpin carries the hairpin as its own curve too,
//!   over `amp` (a **swell**), where the reading hears a dynamic in both
//!   places: a synth listens to no controller, and a sustained sound grows
//!   inside the note.
//! - **The pedal is a lane**, controller 64 on each channel of its staff, and
//!   **a glissando is a note's own curve**, a bend over the note to the one
//!   it slides to -- which a MIDI 1.0 channel cannot carry, so a sequence
//!   with one is MIDI 2.0.
//!
//! A tie is one sound, as the interpreter reads it: the chain is one event,
//! whose `value` is the chain's whole written length -- two tied quarters
//! render as a half, which is what they sound and what a page written back
//! from the event shows.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use clausters_core::envshape::SHAPE_HOLD;
use clausters_core::notation::{
    DynamicsAs, Interpretation, Item, Sheet, heard_beats, levels, performance,
};
use clausters_core::ratio::Ratio;

use super::{Event, EventSequence, MidiSpec};
use crate::multitrack::Automation;

/// The most channels a MIDI 1.0 port has.
const MIDI_CHANNELS: usize = 16;

/// The channel each voice renders on: `(staff, voice)` to its channel, counted
/// from the top staff's first voice.
fn channels(sheet: &Sheet) -> BTreeMap<(usize, usize), usize> {
    sheet
        .staves
        .iter()
        .enumerate()
        .flat_map(|(staff, s)| (0..s.voices.len().max(1)).map(move |voice| (staff, voice)))
        .enumerate()
        .map(|(channel, place)| (place, channel))
        .collect()
}

/// The item `id` of `sheet`, and the written value of the sound it starts:
/// its own, and that of every note it is tied into -- the chain the
/// interpreter reads as one.
fn item_of(sheet: &Sheet, id: u64) -> Option<(&Item, Ratio)> {
    for voice in sheet.staves.iter().flat_map(|s| s.voices.iter()) {
        let Some(at) = voice.items.iter().position(|item| item.id() == id) else {
            continue;
        };
        let mut value = voice.items[at].dur();
        let mut last = at;
        while let Item::Note { tie: true, .. } = voice.items[last] {
            match voice.items.get(last + 1) {
                Some(next) if next.sounds() => {
                    value = value + next.dur();
                    last += 1;
                }
                _ => break,
            }
        }
        return Some((&voice.items[at], value));
    }
    None
}

/// **Render `sheet` under `interp` into a sequence** (see the module).
///
/// # Errors
/// When a spanner names an item that is not on the sheet, as the interpreter
/// refuses it.
pub fn render(sheet: &Sheet, interp: &Interpretation) -> Result<EventSequence, String> {
    let mut sheet = sheet.clone();
    sheet.assign_ids();
    let channel_of = channels(&sheet);
    let played = performance(sheet.clone(), interp)?;
    let notes = &played.notes;

    // An item's pitches are handed out one to each of its notes, in the
    // order the interpreter sounds them, so two notes of one number in a
    // chord still take a pitch each.
    let mut taken: BTreeMap<u64, Vec<bool>> = BTreeMap::new();
    let mut first_event: BTreeMap<u64, u64> = BTreeMap::new();
    let mut items: Vec<Value> = Vec::new();
    let mut events: Vec<Event> = Vec::new();
    for (index, note) in notes.iter().enumerate() {
        let id = index as u64 + 1;
        let channel = channel_of
            .get(&(note.staff, note.voice))
            .copied()
            .unwrap_or(0);
        let mut keys = Map::new();
        keys.insert("midinote".into(), json!(note.pitch));
        keys.insert("dur".into(), json!(note.dur));
        keys.insert("sustain".into(), json!(note.sustain));
        keys.insert("amp".into(), json!(note.amp));
        keys.insert("channel".into(), json!(channel));
        keys.insert("staff".into(), json!(note.staff));
        keys.insert("voice".into(), json!(note.voice));
        if let Some((item, value)) = item_of(&sheet, note.id) {
            let used = taken
                .entry(note.id)
                .or_insert_with(|| vec![false; item.pitches().len()]);
            let written = item
                .pitches()
                .iter()
                .enumerate()
                .find(|(i, pitch)| !used[*i] && pitch.midi() == note.pitch);
            if let Some((i, pitch)) = written {
                used[i] = true;
                keys.insert("pitches".into(), json!([pitch]));
            }
            keys.insert("value".into(), json!(value));
        }
        let marks = &note.marks;
        if !marks.articulations.is_empty() {
            keys.insert("articulations".into(), json!(marks.articulations));
        }
        for (key, mark) in [
            ("dynamic", &marks.dynamic),
            ("ornament", &marks.ornament),
            ("grace", &marks.grace),
            ("stem", &marks.stem),
        ] {
            if let Some(mark) = mark {
                keys.insert(key.into(), json!(mark));
            }
        }
        first_event.entry(note.id).or_insert(id);
        items.push(json!([id, note.id]));
        // a glissando: the pitch bent over the held note to where it slides
        let mut automation = Vec::new();
        if let Some(glide) = note.glide.filter(|g| *g != 0.0) {
            let curve = json!({
                "id": 0,
                "name": "glissando",
                "target": {"bend": true},
                "points": [{"at": 0.0, "value": 0.0}, {"at": note.sustain, "value": glide}],
                "visible": true,
            });
            automation.push(serde_json::from_value(curve).map_err(|why| why.to_string())?);
        }
        // a swell: the level of a note held under a hairpin, moving with it
        if !note.swell.is_empty() {
            let points: Vec<Value> = note
                .swell
                .iter()
                .map(|&(at, factor)| json!({"at": at, "value": note.amp * factor}))
                .collect();
            let curve = json!({
                "id": 0,
                "name": "swell",
                "target": {"control": "amp"},
                "points": points,
                "visible": true,
            });
            automation.push(serde_json::from_value(curve).map_err(|why| why.to_string())?);
        }
        events.push(Event {
            id,
            automation,
            ..Event::new(note.t, Value::Object(keys))
        });
    }
    let glides = notes.iter().any(|n| n.glide.is_some_and(|g| g != 0.0));

    // What is no note's: the sheet, without its items.
    let spanners: Vec<Value> = sheet
        .spanners
        .iter()
        .filter_map(|spanner| {
            Some(json!({
                "kind": spanner.kind,
                "from": first_event.get(&spanner.from)?,
                "to": first_event.get(&spanner.to)?,
            }))
        })
        .collect();
    let mut notation = Map::new();
    notation.insert("grid".into(), json!(sheet.grid));
    notation.insert("key".into(), json!(sheet.key));
    notation.insert(
        "staves".into(),
        sheet
            .staves
            .iter()
            .map(|staff| json!({"clef": staff.clef, "voices": staff.voices.len()}))
            .collect(),
    );
    if !sheet.header.is_empty() {
        notation.insert("header".into(), json!(sheet.header));
    }
    if let Some(page) = &sheet.page {
        notation.insert("page".into(), json!(page));
    }
    notation.insert("spanners".into(), Value::Array(spanners));
    notation.insert("items".into(), Value::Array(items));

    let mut lanes = if interp.dynamics_as == DynamicsAs::Attack {
        Vec::new()
    } else {
        dynamics(&sheet, interp, &channel_of)?
    };
    lanes.extend(pedals(&played.pedals, &channel_of)?);
    let mut sequence = EventSequence {
        next_id: events.len() as u64,
        events,
        automation: lanes,
        // Every curve a render writes is a channel's, which MIDI 1.0 says --
        // as long as there are channels for the voices, and no note bends on
        // its own.
        midi: if glides {
            Some(MidiSpec::Midi2)
        } else {
            (channel_of.len() <= MIDI_CHANNELS).then_some(MidiSpec::Midi1)
        },
        notation: Some(Value::Object(notation)),
        tempo_map: Some(played.tempo_map(interp.beat_unit)),
        ..EventSequence::default()
    };
    sequence.hold();
    Ok(sequence)
}

/// The staves' levels as lanes: one for each channel of each staff that has a
/// level, the lanes of a staff with several voices named as one group.
fn dynamics(
    sheet: &Sheet,
    interp: &Interpretation,
    channel_of: &BTreeMap<(usize, usize), usize>,
) -> Result<Vec<Automation>, String> {
    let mut lanes = Vec::new();
    for level in levels(sheet.clone(), interp)? {
        // each point where it is heard: a repeated stretch twice
        let mut heard: Vec<(f64, f64, bool)> = level
            .points
            .iter()
            .flat_map(|point| {
                let value = (point.amp * 127.0).clamp(0.0, 127.0);
                heard_beats(sheet, interp.beat_unit, point.t)
                    .into_iter()
                    .map(move |at| (at, value, point.ramps))
            })
            .collect();
        heard.sort_by(|a, b| a.0.total_cmp(&b.0));
        let points: Vec<Value> = heard
            .into_iter()
            .map(|(at, value, ramps)| {
                if ramps {
                    json!({"at": at, "value": value})
                } else {
                    json!({"at": at, "value": value, "data": {"shape": SHAPE_HOLD}})
                }
            })
            .collect();
        let voices: Vec<usize> = channel_of
            .iter()
            .filter(|((staff, _), _)| *staff == level.staff)
            .map(|(_, channel)| *channel)
            .collect();
        let group = (voices.len() > 1).then(|| format!("staff {}", level.staff + 1));
        for channel in voices {
            let mut target = json!({"cc": interp.dynamics_cc, "channel": channel});
            if let (Some(group), Some(map)) = (&group, target.as_object_mut()) {
                map.insert("group".into(), json!(group));
            }
            let lane = json!({
                "id": 0,
                "name": "dynamics",
                "target": target,
                "points": points,
                "visible": true,
            });
            lanes.push(serde_json::from_value(lane).map_err(|why| why.to_string())?);
        }
    }
    Ok(lanes)
}

/// The sustain pedal as lanes: controller 64 on each channel of the staff it
/// is written on, pressed and let go.
fn pedals(
    pedals: &[(usize, f64, f64)],
    channel_of: &BTreeMap<(usize, usize), usize>,
) -> Result<Vec<Automation>, String> {
    let mut by_staff: BTreeMap<usize, Vec<(f64, f64)>> = BTreeMap::new();
    for &(staff, down, up) in pedals {
        by_staff.entry(staff).or_default().push((down, up));
    }
    let mut lanes = Vec::new();
    for (staff, mut spans) in by_staff {
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        let points: Vec<Value> = spans
            .iter()
            .flat_map(|(down, up)| {
                [
                    json!({"at": down, "value": 127.0, "data": {"shape": SHAPE_HOLD}}),
                    json!({"at": up, "value": 0.0, "data": {"shape": SHAPE_HOLD}}),
                ]
            })
            .collect();
        for (_, channel) in channel_of.iter().filter(|((s, _), _)| *s == staff) {
            let lane = json!({
                "id": 0,
                "name": "pedal",
                "target": {"cc": 64, "channel": channel},
                "points": points,
                "visible": true,
            });
            lanes.push(serde_json::from_value(lane).map_err(|why| why.to_string())?);
        }
    }
    Ok(lanes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::CurveKind;
    use clausters_core::notation::{Marks, Pitch, Spanner, Staff, Step, Voice};

    fn pitch(step: Step, alter: i32) -> Pitch {
        Pitch {
            step,
            alter,
            octave: 4,
            forced: false,
        }
    }

    fn note(id: u64, pitches: Vec<Pitch>, dur: (i64, i64), marks: Marks) -> Item {
        Item::Note {
            id,
            pitches,
            dur: Ratio::new(dur.0, dur.1),
            tie: false,
            marks,
        }
    }

    fn plain(id: u64) -> Item {
        note(id, vec![pitch(Step::C, 0)], (1, 4), Marks::default())
    }

    fn sheet(voices: Vec<Vec<Item>>, spanners: Vec<Spanner>) -> Sheet {
        Sheet {
            staves: vec![Staff {
                clef: "G2".into(),
                voices: voices.into_iter().map(|items| Voice { items }).collect(),
                ..Staff::default()
            }],
            spanners,
            key: "C".into(),
            ..Sheet::default()
        }
    }

    #[test]
    fn the_page_plays_its_repeats_its_tempo_its_pedal_and_its_slides() {
        let mut score = sheet(vec![(1..=8).map(plain).collect()], Vec::new());
        score.grid.barlines = vec![(0, "rptend".into())];
        score.spanners = vec![
            Spanner {
                kind: "pedal".into(),
                from: 5,
                to: 6,
            },
            Spanner {
                kind: "gliss".into(),
                from: 7,
                to: 8,
            },
        ];
        if let Item::Note { pitches, .. } = &mut score.staves[0].voices[0].items[7] {
            pitches[0] = pitch(Step::E, 0);
        }
        score.controls = vec![clausters_core::notation::Control {
            kind: "tempo".into(),
            on: 5,
            text: "Adagio".into(),
            bpm: Some(60.0),
        }];
        let sequence = render(&score, &Interpretation::default()).unwrap();
        // the first bar twice, then the second
        assert_eq!(sequence.events.len(), 12);
        let tempo = sequence.tempo_map.as_ref().expect("a tempo map");
        // eight beats at two a second, then one a second
        assert!((tempo.secs_at(9.0) - 5.0).abs() < 1e-9);
        let pedal = sequence
            .automation
            .iter()
            .find(|lane| lane.target.0["cc"] == 64)
            .expect("a pedal lane");
        assert_eq!(pedal.points.len(), 2);
        let slide = sequence
            .events
            .iter()
            .find(|e| !e.automation.is_empty())
            .expect("a note that slides");
        assert_eq!(slide.automation[0].target.0, json!({"bend": true}));
        // straight in semitones, which a bend makes geometric in frequency
        let ends: Vec<f64> = slide.automation[0].points.iter().map(|p| p.value).collect();
        assert_eq!(ends, vec![0.0, 4.0], "C to E");
        assert_eq!(sequence.midi, Some(MidiSpec::Midi2));
    }

    /// **A note held under a hairpin swells**: its own curve over `amp`, from
    /// its level at the attack to where the hairpin has taken it when it is
    /// let go -- what a synth hears, since it listens to no controller.
    #[test]
    fn a_held_note_under_a_hairpin_carries_its_swell() {
        let mut score = sheet(vec![(1..=3).map(plain).collect()], Vec::new());
        if let Item::Note { tie, .. } = &mut score.staves[0].voices[0].items[0] {
            *tie = true;
        }
        score.spanners = vec![Spanner {
            kind: "crescendo".into(),
            from: 1,
            to: 3,
        }];
        let sequence = render(&score, &Interpretation::default()).unwrap();
        let held = &sequence.events[0];
        let swell = held
            .automation
            .iter()
            .find(|c| c.target.0 == json!({"control": "amp"}))
            .expect("a swell");
        let amp = held.keys()["amp"].as_f64().unwrap();
        assert_eq!(swell.points[0].value, amp, "from the attack's level");
        assert!(swell.points.last().unwrap().value > amp, "and growing");
        // and the lane the controller hears is still there for a MIDI port
        assert!(
            sequence
                .automation
                .iter()
                .any(|lane| lane.target.0["cc"] == 11)
        );
    }

    #[test]
    fn a_note_is_an_event_that_still_says_what_it_is_on_the_page() {
        // an F flat with a staccato as a triplet eighth, then a chord
        let staccato = Marks {
            articulations: vec!["stacc".into()],
            ..Marks::default()
        };
        let score = sheet(
            vec![vec![
                note(1, vec![pitch(Step::F, -1)], (1, 12), staccato),
                note(
                    2,
                    vec![pitch(Step::C, 0), pitch(Step::G, 0)],
                    (1, 4),
                    Marks::default(),
                ),
            ]],
            Vec::new(),
        );
        let sequence = render(&score, &Interpretation::default()).unwrap();
        assert_eq!(sequence.events.len(), 3, "a chord is an event per note");
        let first = sequence.events[0].keys();
        assert_eq!(first["midinote"], 64, "it sounds an E");
        assert_eq!(
            first["pitches"],
            json!([{"step": "f", "alter": -1, "octave": 4}])
        );
        assert_eq!(first["value"], json!([1, 12]));
        assert_eq!(first["articulations"], json!(["stacc"]));
        assert!(first["sustain"].as_f64().unwrap() < first["dur"].as_f64().unwrap());
        assert_eq!(
            (first["staff"].as_i64(), first["voice"].as_i64()),
            (Some(0), Some(0))
        );
        // the chord's notes share an item, and each has a pitch of its own
        let spelled: Vec<&str> = sequence.events[1..]
            .iter()
            .map(|e| e.data.0["pitches"][0]["step"].as_str().unwrap())
            .collect();
        assert_eq!(spelled, vec!["c", "g"]);
        let section = sequence.notation.as_ref().unwrap();
        assert_eq!(section["items"], json!([[1, 1], [2, 2], [3, 2]]));
        assert_eq!(section["key"], "C");
        assert_eq!(section["staves"], json!([{"clef": "G2", "voices": 1}]));
        // every id is the sequence's own, and none is handed out twice
        assert_eq!(sequence.next_id, 3);
    }

    #[test]
    fn a_tie_is_one_event_of_the_whole_written_length() {
        let tied = Item::Note {
            id: 1,
            pitches: vec![pitch(Step::C, 0)],
            dur: Ratio::new(1, 4),
            tie: true,
            marks: Marks::default(),
        };
        let score = sheet(vec![vec![tied, plain(2), plain(3)]], Vec::new());
        let sequence = render(&score, &Interpretation::default()).unwrap();
        assert_eq!(sequence.events.len(), 2, "the tied pair attacks once");
        let first = sequence.events[0].keys();
        assert_eq!(first["value"], json!([1, 2]), "two quarters, a half");
        assert_eq!(first["dur"], 2.0);
        assert!(first.get("tie").is_none(), "the chain ended inside it");
    }

    #[test]
    fn a_voice_is_a_channel_and_a_staffs_dynamic_is_a_lane_of_each() {
        let forte = Marks {
            dynamic: Some("f".into()),
            ..Marks::default()
        };
        let piano = Marks {
            dynamic: Some("p".into()),
            ..Marks::default()
        };
        // two voices on one staff; a crescendo from p to f over the first
        let score = sheet(
            vec![
                vec![
                    note(1, vec![pitch(Step::C, 0)], (1, 4), piano),
                    plain(2),
                    note(3, vec![pitch(Step::C, 0)], (1, 4), forte),
                ],
                vec![plain(4), plain(5), plain(6)],
            ],
            vec![Spanner {
                kind: "crescendo".into(),
                from: 1,
                to: 3,
            }],
        );
        let sequence = render(&score, &Interpretation::default()).unwrap();
        let channel = |id: u64| {
            let event = sequence.events.iter().find(|e| {
                sequence.notation.as_ref().unwrap()["items"]
                    .as_array()
                    .unwrap()
                    .contains(&json!([e.id, id]))
            });
            event.unwrap().data.0["channel"].as_i64().unwrap()
        };
        assert_eq!((channel(1), channel(4)), (0, 1));
        // the level of the staff, on both of its channels, as one group
        assert_eq!(sequence.automation.len(), 2);
        for (lane, channel) in sequence.automation.iter().zip([0, 1]) {
            let target = &lane.target.0;
            assert_eq!(CurveKind::of(target), CurveKind::Cc(11));
            assert_eq!(target["channel"], channel);
            assert_eq!(target["group"], "staff 1");
            // a ramp from p to f: 0.05 and 0.17 of full scale
            let said: Vec<(f64, f64)> = lane.points.iter().map(|p| (p.at, p.value)).collect();
            assert_eq!(said, vec![(0.0, 0.05 * 127.0), (2.0, 0.17 * 127.0)]);
            assert!(lane.points[0].data.is_empty(), "the first stretch ramps");
            assert_ne!(lane.id.0, 0, "a lane has an identity of the sequence's");
        }
        // the hairpin's two ends are events now
        let spanner = &sequence.notation.as_ref().unwrap()["spanners"][0];
        assert_eq!(spanner["kind"], "crescendo");
        assert_eq!(
            (channel_of_event(&sequence, spanner["from"].as_u64().unwrap())),
            0
        );
        assert_eq!(sequence.midi, Some(MidiSpec::Midi1));
        // heard in the attack alone, a dynamic writes no lane
        let attack = Interpretation {
            dynamics_as: DynamicsAs::Attack,
            ..Interpretation::default()
        };
        assert!(render(&score, &attack).unwrap().automation.is_empty());
    }

    fn channel_of_event(sequence: &EventSequence, id: u64) -> i64 {
        sequence
            .events
            .iter()
            .find(|e| e.id == id)
            .and_then(|e| e.data.0["channel"].as_i64())
            .expect("an event")
    }

    #[test]
    fn a_staff_with_one_voice_has_its_lane_and_no_group() {
        let forte = Marks {
            dynamic: Some("f".into()),
            ..Marks::default()
        };
        let score = sheet(
            vec![vec![
                note(1, vec![pitch(Step::C, 0)], (1, 4), forte),
                plain(2),
            ]],
            Vec::new(),
        );
        let sequence = render(&score, &Interpretation::default()).unwrap();
        assert_eq!(sequence.automation.len(), 1);
        let target = &sequence.automation[0].target.0;
        assert!(target.get("group").is_none());
        // a level that steps holds its value: the point says so
        assert_eq!(
            sequence.automation[0].points[0].data.0["shape"],
            json!(SHAPE_HOLD)
        );
    }
}
