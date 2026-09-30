use serde_json::json;

use super::*;
use crate::history::History;

fn at(at: f64) -> Event {
    Event::new(at, Value::Null)
}

fn note(at: f64, midinote: i64) -> Event {
    Event::new(at, json!({"midinote": midinote, "instrument": "bell"}))
}

fn set(events: Vec<Event>) -> Opaque {
    payload(&EventsIntent::SetEvents { events })
}

fn beats(sequence: &EventSequence) -> Vec<f64> {
    sequence.events.iter().map(|e| e.at.0).collect()
}

#[test]
fn a_sequence_edited_through_a_history_inverts_to_the_events_it_started_from() {
    let mut history = History::new();
    let roll = history.register(EVENTS);
    let mut sequence = EventSequence::new(vec![at(0.0), at(1.0)]);
    let before = sequence.clone();

    let applied = history.apply(
        roll,
        &mut sequence,
        &set(vec![at(0.0), at(1.5), at(2.0)]),
        "edit the notes",
    );
    assert!(applied.applied);
    assert_eq!(sequence.events.len(), 3);

    for (structure, payload) in history.undo().expect("something to undo").legs {
        assert_eq!(structure, roll);
        sequence.apply(&payload);
    }
    assert_eq!(
        sequence.events, before.events,
        "where it started, ids and all"
    );
    assert_eq!(
        sequence.next_id, 4,
        "and the ids it minted are not minted again"
    );
}

#[test]
fn an_event_gets_an_id_and_keeps_it() {
    let mut sequence = EventSequence::new(vec![note(0.0, 60), note(1.0, 62)]);
    let ids: Vec<u64> = sequence.events.iter().map(|e| e.id).collect();
    assert_eq!(ids, vec![1, 2]);

    // A whole list without ids: the unchanged note keeps its id, the new one
    // gets the next, and the removed one's id is never handed out again.
    let change = sequence
        .edit(EventsIntent::SetEvents {
            events: vec![note(1.0, 62), note(2.0, 64)],
        })
        .unwrap();
    assert!(change.applied);
    let ids: Vec<u64> = sequence.events.iter().map(|e| e.id).collect();
    assert_eq!(ids, vec![2, 3]);
}

/// The defect ids end: removing note k handed note k+1 the data of note k,
/// because an edit named its event by position.
#[test]
fn removing_an_event_leaves_its_neighbours_theirs() {
    let mut sequence = EventSequence::new(vec![note(0.0, 60), note(1.0, 62), note(2.0, 64)]);
    sequence.edit(EventsIntent::Remove { id: 1 }).unwrap();
    sequence
        .edit(EventsIntent::Move {
            id: 2,
            at: Beat(3.0),
        })
        .unwrap();
    assert_eq!(beats(&sequence), vec![2.0, 3.0]);
    assert_eq!(sequence.get(2).unwrap().data.0["midinote"], json!(62));
    assert_eq!(sequence.get(3).unwrap().data.0["midinote"], json!(64));
}

#[test]
fn a_key_is_written_with_its_familys_coherence() {
    let mut sequence = EventSequence::new(vec![Event::new(
        0.0,
        json!({"freq": 440.0, "midinote": 69}),
    )]);
    sequence
        .edit(EventsIntent::Set {
            id: 1,
            key: "midinote".into(),
            value: json!(72),
        })
        .unwrap();
    let freq = sequence.get(1).unwrap().data.0["freq"].as_f64().unwrap();
    assert!((freq - 523.251_130_601_197_3).abs() < 1e-9);
}

#[test]
fn an_add_says_which_id_it_gave_and_keeps_the_order_it_was_made_in() {
    let mut sequence = EventSequence::new(vec![note(1.0, 60)]);
    let change = sequence
        .edit(EventsIntent::Add {
            event: note(1.0, 67),
        })
        .unwrap();
    assert_eq!(change.added, Some(2));
    assert_eq!(sequence.events[1].id, 2, "after the event already at 1.0");
    assert!(sequence.edit(EventsIntent::Remove { id: 9 }).is_err());
}

#[test]
fn a_resend_is_not_an_edit_and_leaves_no_entry() {
    let mut history = History::new();
    let roll = history.register(EVENTS);
    let mut sequence = EventSequence::new(vec![at(0.0)]);

    let applied = history.apply(roll, &mut sequence, &set(vec![at(0.0)]), "edit the notes");
    assert!(!applied.applied);
    assert_eq!(history.len(), 0);
}

#[test]
fn a_bare_list_still_reads_and_the_tempo_map_travels() {
    let sequence: EventSequence =
        serde_json::from_value(json!([{"at": 1.0, "data": {"midinote": 60}}])).unwrap();
    assert_eq!(sequence.events[0].id, 1);

    let mut sequence = sequence;
    let map = TempoMap::new(2.0);
    sequence
        .edit(EventsIntent::Tempo {
            tempo_map: Some(map.clone()),
        })
        .unwrap();
    let back: EventSequence =
        serde_json::from_str(&serde_json::to_string(&sequence).unwrap()).unwrap();
    assert_eq!(back.tempo_map, Some(map));
    assert_eq!(back, sequence);
}

#[test]
fn the_duration_is_where_the_last_note_stops() {
    let sequence = EventSequence::new(vec![
        Event::new(0.0, json!({"dur": 1.0, "legato": 1.0})),
        Event::new(2.0, json!({"sustain": 3.0})),
    ]);
    assert_eq!(sequence.duration(), 5.0);
}

#[test]
fn an_edit_in_another_vocabulary_is_refused_with_the_sequence_as_it_stands() {
    let mut sequence = EventSequence::new(vec![at(3.0)]);
    let applied = sequence.apply(&crate::points::payload(
        &crate::points::PointsIntent::SetPoints { points: Vec::new() },
    ));
    assert!(!applied.applied);
    assert!(applied.reason.is_some());
}

#[test]
fn the_verb_door_answers_in_json() {
    let mut sequence = EventSequence::new(vec![note(0.0, 60)]);
    let added: Value = serde_json::from_str(&call_json(
        &mut sequence,
        r#"{"verb":"apply","intent":{"intent":"add","event":{"at":2.0,"data":{"midinote":64}}}}"#,
    ))
    .unwrap();
    assert_eq!(added["id"], json!(2));
    assert_eq!(added["applied"], json!(true));
    let len: Value = serde_json::from_str(&call_json(&mut sequence, r#"{"verb":"len"}"#)).unwrap();
    assert_eq!(len["len"], json!(2));
    let undone: Value = serde_json::from_str(&call_json(
        &mut sequence,
        &json!({"verb": "apply", "intent": added["current"]}).to_string(),
    ))
    .unwrap();
    assert_eq!(undone["applied"], json!(true));
    assert_eq!(sequence.events.len(), 1);
    let bad: Value = serde_json::from_str(&call_json(&mut sequence, r#"{"verb":"nope"}"#)).unwrap();
    assert!(bad["error"].is_string());
}

#[test]
fn coalescing_is_per_verb_and_event() {
    let key = |intent: EventsIntent| coalesce_key(&payload(&intent));
    assert_eq!(
        key(EventsIntent::Move {
            id: 3,
            at: Beat(1.0)
        }),
        Some("events:move:3".into())
    );
    assert_eq!(key(EventsIntent::Remove { id: 3 }), None);
    assert_eq!(
        key(EventsIntent::SetEvents { events: Vec::new() }),
        Some(EVENTS.into())
    );
}

#[test]
fn a_sequence_goes_to_midi_and_back() {
    let mut sequence = EventSequence::new(vec![
        Event::new(
            0.0,
            json!({"midinote": 60, "velocity": 100, "sustain": 1.0}),
        ),
        Event::new(
            1.0,
            json!({"type": "midi", "midicmd": "cc", "cc": 7, "value": 99}),
        ),
        Event::new(2.0, json!({"type": "osc", "addr": "/x"})),
    ]);
    sequence.tempo_map = Some(TempoMap::new(4.0));
    let (events, tempo) = sequence.to_midi(96);
    assert_eq!(
        events,
        vec![
            (0, vec![0x90, 60, 100]),
            (96, vec![0x80, 60, 0]),
            (96, vec![0xB0, 7, 99]),
        ],
        "the osc event has no MIDI spelling"
    );
    assert_eq!(tempo, vec![(0, 250_000)]);

    let back = EventSequence::from_midi(96, &events, &tempo).unwrap();
    assert_eq!(back.events.len(), 1, "the note; the CC is a lane now");
    assert_eq!(back.events[0].data.0["sustain"], json!(1.0));
    assert_eq!(back.lanes[0].target.0, json!({"cc": 7, "channel": 0}));
    assert_eq!(
        (back.lanes[0].points[0].at, back.lanes[0].points[0].value),
        (1.0, 99.0)
    );
    assert_eq!(back.tempo_map, Some(TempoMap::new(4.0)));
    let untimed = EventSequence::from_midi(96, &events, &[]).unwrap();
    assert_eq!(
        untimed.tempo_map,
        Some(TempoMap::new(2.0)),
        "a file's default 120"
    );
    assert!(mutates(r#"{"verb":"loadmidi"}"#) && !mutates(r#"{"verb":"midi"}"#));
}

#[test]
fn a_lane_and_a_notes_curve_are_set_whole_and_removed() {
    use crate::NodeId;
    use crate::multitrack::Automation;
    let mut sequence = EventSequence::new(vec![Event::new(0.0, json!({"midinote": 60}))]);
    let note = sequence.events[0].id;
    let mut cc = Automation::new(NodeId(0), Opaque(json!({"cc": 74})));
    cc.points.push(crate::Point {
        at: 0.0,
        value: 0.5,
        data: Opaque::none(),
    });
    let lane = sequence
        .edit(EventsIntent::Lane {
            automation: cc.clone(),
        })
        .unwrap()
        .added
        .unwrap();
    assert!(lane > note, "a curve's id comes off the events' counter");
    cc.id = NodeId(lane);
    cc.points.push(crate::Point {
        at: 1.0,
        value: 1.0,
        data: Opaque::none(),
    });
    sequence
        .edit(EventsIntent::Lane {
            automation: cc.clone(),
        })
        .unwrap();
    assert_eq!(sequence.lanes.len(), 1, "set whole, not added again");
    assert_eq!(sequence.lanes[0].points.len(), 2);

    let bend = Automation::new(NodeId(0), Opaque(json!({"bend": true})));
    let curve = sequence
        .edit(EventsIntent::Expression {
            id: note,
            automation: bend,
        })
        .unwrap()
        .added
        .unwrap();
    assert_eq!(sequence.events[0].expression[0].id, NodeId(curve));
    sequence
        .edit(EventsIntent::RemoveExpression {
            id: note,
            lane: NodeId(curve),
        })
        .unwrap();
    assert!(sequence.events[0].expression.is_empty());
    sequence
        .edit(EventsIntent::RemoveLane { lane: NodeId(lane) })
        .unwrap();
    assert!(sequence.lanes.is_empty());
    assert!(
        sequence
            .edit(EventsIntent::RemoveLane { lane: NodeId(99) })
            .is_err()
    );
}

/// **A MIDI spec admits the curves it can say**: MIDI 1.0 a note's pressure
/// and not its bend, MPE a bend and not a per-note CC, 2.0 both; no spec
/// admits a lane over a bare control; and a spec the curves already there
/// cannot be said in is refused, the sequence left as it was.
#[test]
fn a_midi_spec_admits_the_curves_it_can_say() {
    let curve = |target: Value| Automation::new(NodeId(0), Opaque(target));
    let mut sequence = EventSequence::new(vec![note(0.0, 60)]);
    let id = sequence.events[0].id;
    let expression = |target: Value| EventsIntent::Expression {
        id,
        automation: curve(target),
    };
    sequence
        .edit(EventsIntent::Midi {
            midi: Some(MidiSpec::Midi1),
        })
        .unwrap();
    assert!(sequence.edit(expression(json!({"pressure": true}))).is_ok());
    let refused = sequence
        .edit(expression(json!({"bend": true})))
        .unwrap_err();
    assert!(refused.contains("MIDI 1.0"), "{refused}");
    assert!(
        sequence
            .edit(EventsIntent::Lane {
                automation: curve(json!({"control": "cutoff"}))
            })
            .is_err(),
        "a bare control has no MIDI spelling"
    );
    assert!(
        sequence
            .edit(EventsIntent::Lane {
                automation: curve(json!({"cc": 74, "control": "cutoff"}))
            })
            .is_ok(),
        "a CC is one, whatever it reaches on the server"
    );

    let mpe = MidiSpec::Mpe {
        upper: false,
        members: 15,
    };
    sequence
        .edit(EventsIntent::Midi { midi: Some(mpe) })
        .unwrap();
    assert!(sequence.edit(expression(json!({"bend": true}))).is_ok());
    assert!(sequence.edit(expression(json!({"cc": 1}))).is_err());
    sequence
        .edit(EventsIntent::Midi {
            midi: Some(MidiSpec::Midi2),
        })
        .unwrap();
    assert!(sequence.edit(expression(json!({"cc": 1}))).is_ok());

    let before = sequence.clone();
    let back = sequence
        .edit(EventsIntent::Midi {
            midi: Some(MidiSpec::Midi1),
        })
        .unwrap_err();
    assert!(back.contains("bend"), "{back}");
    assert_eq!(sequence, before, "a refused spec changes nothing");
    sequence.edit(EventsIntent::Midi { midi: None }).unwrap();
    assert_eq!(sequence.midi, None, "for the server, everything is legal");
}

/// **A spec is written as the standards name it**, and a file read is MIDI 1.0.
#[test]
fn a_midi_spec_is_written_as_its_name() {
    assert_eq!(serde_json::to_value(MidiSpec::Midi1).unwrap(), json!("1.0"));
    assert_eq!(serde_json::to_value(MidiSpec::Midi2).unwrap(), json!("2.0"));
    assert_eq!(
        serde_json::from_value::<MidiSpec>(json!({"mpe": {"upper": true}})).unwrap(),
        MidiSpec::Mpe {
            upper: true,
            members: 15
        }
    );
    let read = EventSequence::from_midi(480, &[(0, vec![0x90, 60, 100])], &[]).unwrap();
    assert_eq!(read.midi, Some(MidiSpec::Midi1));
}

/// Beats of `ppq` 4, as a file's ticks.
fn ticks(messages: &[(f64, Vec<u8>)]) -> Vec<(u32, Vec<u8>)> {
    messages
        .iter()
        .map(|(beat, bytes)| ((beat * 4.0) as u32, bytes.clone()))
        .collect()
}

/// **A channel's streams are lanes, and a note's pressure its own**: a CC,
/// the bend (through the channel's RPN 0 range) and channel pressure on
/// channel 2 are that channel's lanes, each message a step; poly pressure is
/// the expression of the note on its key; the RPN is consumed and a program
/// change stays an event.
#[test]
fn a_files_streams_are_lanes_and_a_notes_pressure_its_own() {
    let file = ticks(&[
        (0.0, vec![0xB2, 101, 0]),
        (0.0, vec![0xB2, 100, 0]),
        (0.0, vec![0xB2, 6, 12]),
        (0.0, vec![0xC2, 5]),
        (0.0, vec![0x92, 60, 100]),
        (0.0, vec![0xB2, 7, 100]),
        (1.0, vec![0xB2, 7, 50]),
        (1.0, vec![0xE2, 0x00, 0x60]),
        (1.5, vec![0xD2, 127]),
        (0.5, vec![0xA2, 60, 64]),
        (2.0, vec![0x82, 60, 0]),
    ]);
    let s = EventSequence::from_midi(4, &file, &[]).unwrap();
    assert_eq!(s.midi, Some(MidiSpec::Midi1));
    let lane = |target: Value| s.lanes.iter().find(|l| l.target.0 == target).unwrap();
    let volume = lane(json!({"cc": 7, "channel": 2}));
    let points: Vec<(f64, f64)> = volume.points.iter().map(|p| (p.at, p.value)).collect();
    assert_eq!(points, [(0.0, 100.0), (1.0, 50.0)]);
    // 0x60 << 7 is 12288, a half of the way up: 6 of the channel's 12.
    assert_eq!(
        lane(json!({"bend": true, "channel": 2})).points[0].value,
        6.0
    );
    assert_eq!(
        lane(json!({"pressure": true, "channel": 2})).points[0].value,
        1.0
    );
    let note = s
        .events
        .iter()
        .find(|e| e.keys().get("midinote").is_some())
        .unwrap();
    assert_eq!(note.expression[0].target.0, json!({"pressure": true}));
    assert_eq!(
        note.expression[0].points[0].at, 0.5,
        "from the note's start"
    );
    let kept: Vec<String> = s
        .events
        .iter()
        .filter_map(|e| {
            e.keys()
                .get("midicmd")
                .and_then(Value::as_str)
                .map(String::from)
        })
        .collect();
    assert_eq!(kept, ["program"], "the RPN is consumed, the program kept");
    assert!(s.lanes.iter().chain(&note.expression).all(|c| c.id.0 != 0));
}

/// **An MPE zone's member channels are its notes'**: the configuration
/// declares the lower zone, each member channel's bend (48 semitones by
/// default), pressure and CC 74 are the expression of the note on it -- the
/// first sent just before its note-on -- and the master's bend is a lane over
/// the whole zone.
#[test]
fn an_mpe_zones_members_are_its_notes() {
    let file = ticks(&[
        (0.0, vec![0xB0, 101, 0]),
        (0.0, vec![0xB0, 100, 6]),
        (0.0, vec![0xB0, 6, 15]),
        (0.0, vec![0xE0, 0x00, 0x50]),
        (0.0, vec![0xE1, 0x00, 0x48]),
        (0.0, vec![0x91, 60, 100]),
        (0.0, vec![0xE2, 0x00, 0x40]),
        (0.0, vec![0x92, 64, 100]),
        (1.0, vec![0xD2, 64]),
        (1.0, vec![0xB2, 74, 127]),
        (2.0, vec![0x81, 60, 0]),
        (2.0, vec![0x82, 64, 0]),
    ]);
    let s = EventSequence::from_midi(4, &file, &[]).unwrap();
    assert_eq!(
        s.midi,
        Some(MidiSpec::Mpe {
            upper: false,
            members: 15
        })
    );
    let notes: Vec<&Event> = s.events.iter().collect();
    assert_eq!(notes.len(), 2, "no message is left an event");
    let bend = &notes[0].expression[0];
    assert_eq!(bend.target.0, json!({"bend": true}));
    // 0x48 << 7 is 9216: 1024 of 8192 up, an eighth of 48.
    assert_eq!(bend.points[0].value, 6.0);
    let second: Vec<Value> = notes[1]
        .expression
        .iter()
        .map(|c| c.target.0.clone())
        .collect();
    assert!(second.contains(&json!({"pressure": true})));
    assert!(second.contains(&json!({"timbre": true})));
    let master = &s.lanes[0];
    assert_eq!(master.target.0, json!({"bend": true}), "the whole zone's");
}

/// **What is read is what is written**: a sequence's lanes as its channels'
/// messages, a note's pressure as poly pressure, and a ramp sampled where the
/// message changes; in MPE a member channel per note, the configuration first
/// and a note's first bend before its on.
#[test]
fn a_sequence_writes_its_curves_as_the_spec_says_them() {
    let curve = |target: Value, points: &[(f64, f64, i32)]| {
        let mut c = Automation::new(NodeId(0), Opaque(target));
        c.points = points
            .iter()
            .map(|(at, value, shape)| crate::Point {
                at: *at,
                value: *value,
                data: Opaque(json!({"shape": shape})),
            })
            .collect();
        c
    };
    let mut s = EventSequence::new(vec![note(0.0, 60)]);
    s.edit(EventsIntent::Midi {
        midi: Some(MidiSpec::Midi1),
    })
    .unwrap();
    s.edit(EventsIntent::Lane {
        automation: curve(
            json!({"cc": 1, "channel": 0}),
            &[(0.0, 0.0, 1), (1.0, 127.0, 0)],
        ),
    })
    .unwrap();
    let id = s.events[0].id;
    s.edit(EventsIntent::Expression {
        id,
        automation: curve(json!({"pressure": true}), &[(0.0, 0.5, 0)]),
    })
    .unwrap();
    let (written, _) = s.to_midi(64);
    let ccs: Vec<&Vec<u8>> = written
        .iter()
        .filter(|(_, b)| b[0] == 0xB0)
        .map(|(_, b)| b)
        .collect();
    assert!(ccs.len() > 10, "the ramp is sampled: {}", ccs.len());
    assert_eq!(ccs.last().unwrap()[2], 127);
    assert!(
        written.iter().any(|(_, b)| *b == vec![0xA0, 60, 64]),
        "the note's pressure is poly pressure"
    );

    let mut mpe = EventSequence::new(vec![note(0.0, 60), note(0.0, 64)]);
    mpe.edit(EventsIntent::Midi {
        midi: Some(MidiSpec::Mpe {
            upper: false,
            members: 15,
        }),
    })
    .unwrap();
    let first = mpe.events[0].id;
    mpe.edit(EventsIntent::Expression {
        id: first,
        automation: curve(json!({"bend": true}), &[(0.0, 6.0, 0)]),
    })
    .unwrap();
    let (written, _) = mpe.to_midi(64);
    assert_eq!(
        written[..3]
            .iter()
            .map(|(_, b)| b.clone())
            .collect::<Vec<_>>(),
        [vec![0xB0, 101, 0], vec![0xB0, 100, 6], vec![0xB0, 6, 15]]
    );
    let ons: Vec<u8> = written
        .iter()
        .filter(|(_, b)| b[0] & 0xF0 == 0x90)
        .map(|(_, b)| b[0] & 0x0F)
        .collect();
    assert_eq!(ons.len(), 2);
    assert_ne!(ons[0], ons[1], "a member channel each");
    assert!(ons.iter().all(|c| (1..=15).contains(c)));
    let bend = written
        .iter()
        .position(|(_, b)| b[0] & 0xF0 == 0xE0)
        .unwrap();
    let on = written
        .iter()
        .position(|(_, b)| b[0] & 0xF0 == 0x90 && b[1] == 60)
        .unwrap();
    assert!(bend < on, "the note's first bend before its on");
    assert_eq!(
        written[bend].1,
        vec![0xE0 | ons[0], 0x00, 0x48],
        "6 of 48 up"
    );

    // And back: the file read is the sequence written.
    let back = EventSequence::from_midi(64, &written, &[]).unwrap();
    assert_eq!(back.midi, mpe.midi);
    let bent = back
        .events
        .iter()
        .find(|e| e.keys().get("midinote").and_then(Value::as_f64) == Some(60.0))
        .unwrap();
    assert_eq!(bent.expression[0].points[0].value, 6.0);
}

/// **A MIDI 2.0 sequence is its clip's packets, and back**: a note at 16-bit
/// velocity, its bend as a per-note pitch bend, its timbre as the registered
/// per-note controller 74 and a CC as an assignable one, a lane as a 32-bit
/// channel controller, the tempo as a Set Tempo -- and read back, the same
/// sequence, MIDI 2.0.
#[test]
fn a_midi2_sequence_is_its_clips_packets_and_back() {
    let curve = |target: Value, value: f64| {
        let mut c = Automation::new(NodeId(0), Opaque(target));
        c.points = vec![crate::Point {
            at: 0.0,
            value,
            data: Opaque(json!({"shape": 0})),
        }];
        c
    };
    let mut s = EventSequence::new(vec![Event::new(
        0.0,
        json!({"midinote": 60, "velocity": 100, "sustain": 1.0}),
    )]);
    s.tempo_map = Some(TempoMap::new(2.0));
    s.edit(EventsIntent::Midi {
        midi: Some(MidiSpec::Midi2),
    })
    .unwrap();
    let id = s.events[0].id;
    for (target, value) in [
        (json!({"bend": true}), 12.0),
        (json!({"timbre": true}), 0.5),
        (json!({"cc": 1}), 64.0),
    ] {
        s.edit(EventsIntent::Expression {
            id,
            automation: curve(target, value),
        })
        .unwrap();
    }
    s.edit(EventsIntent::Lane {
        automation: curve(json!({"cc": 7, "channel": 0}), 100.0),
    })
    .unwrap();
    let packets = s.to_ump(480);
    let status = |words: &[u32]| (words[0] >> 28, (words[0] >> 20) & 0xF);
    let kinds: Vec<(u32, u32)> = packets.iter().map(|(_, w)| status(w)).collect();
    assert!(kinds.iter().any(|k| k.0 == 0xD), "a Set Tempo");
    assert!(kinds.contains(&(0x4, 0x9)), "the note on");
    assert!(kinds.contains(&(0x4, 0x6)), "a per-note pitch bend");
    assert!(kinds.contains(&(0x4, 0x0)), "the registered per-note 74");
    assert!(
        kinds.contains(&(0x4, 0x1)),
        "an assignable per-note controller"
    );
    assert!(kinds.contains(&(0x4, 0xB)), "the channel's CC");
    let bend = packets
        .iter()
        .find(|(_, w)| status(w) == (0x4, 0x6))
        .unwrap();
    assert_eq!(bend.1[1], 0xA000_0000, "12 of 48 semitones up");

    let back = EventSequence::from_ump(480, &packets).unwrap();
    assert_eq!(back.midi, Some(MidiSpec::Midi2));
    assert_eq!(back.tempo_map, Some(TempoMap::new(2.0)));
    assert_eq!(back.events.len(), 1);
    let keys = back.events[0].keys();
    assert_eq!(keys["midinote"], json!(60.0));
    assert!((keys["velocity"].as_f64().unwrap() - 100.0).abs() < 0.01);
    let value_of = |target: Value| {
        back.events[0]
            .expression
            .iter()
            .find(|c| c.target.0 == target)
            .map(|c| c.points[0].value)
    };
    assert_eq!(value_of(json!({"bend": true})), Some(12.0));
    assert!((value_of(json!({"timbre": true})).unwrap() - 0.5).abs() < 1e-6);
    assert!((value_of(json!({"cc": 1})).unwrap() - 64.0).abs() < 1e-4);
    assert_eq!(back.lanes[0].target.0, json!({"cc": 7, "channel": 0}));
    assert!((back.lanes[0].points[0].value - 100.0).abs() < 1e-4);
}
