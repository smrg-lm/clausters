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
    assert_eq!(back.events.len(), 2);
    assert_eq!(back.events[0].data.0["sustain"], json!(1.0));
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
