use super::*;
use crate::history::History;

fn at(at: f64) -> Event {
    Event {
        at,
        data: Opaque::none(),
    }
}

fn set(events: Vec<Event>) -> Opaque {
    payload(&EventsIntent::SetEvents { events })
}

#[test]
fn a_timeline_edited_through_a_history_inverts_to_the_events_it_started_from() {
    let mut history = History::new();
    let roll = history.register(EVENTS);
    let mut events = Events::new(vec![at(0.0), at(1.0)]);

    let applied = history.apply(
        roll,
        &mut events,
        &set(vec![at(0.0), at(1.5), at(2.0)]),
        "edit the notes",
    );
    assert!(applied.applied);
    assert_eq!(events.0.len(), 3);

    for (structure, payload) in history.undo().expect("something to undo").legs {
        assert_eq!(structure, roll);
        events.apply(&payload);
    }
    assert_eq!(events.0, vec![at(0.0), at(1.0)], "where it started");
}

#[test]
fn an_events_payload_is_carried_and_never_read() {
    let mut events = Events::new(Vec::new());
    let note = Event {
        at: 2.0,
        data: Opaque(serde_json::json!({"pitch": 60, "velocity": 90})),
    };
    assert!(events.apply(&set(vec![note.clone()])).applied);
    assert_eq!(events.0, vec![note], "verbatim, fields and all");
}

#[test]
fn a_resend_is_not_an_edit_and_leaves_no_entry() {
    let mut history = History::new();
    let roll = history.register(EVENTS);
    let mut events = Events::new(vec![at(0.0)]);

    let applied = history.apply(roll, &mut events, &set(vec![at(0.0)]), "edit the notes");
    assert!(!applied.applied);
    assert_eq!(history.len(), 0);
}

#[test]
fn an_edit_in_another_vocabulary_is_refused_with_the_timeline_as_it_stands() {
    let mut events = Events::new(vec![at(3.0)]);
    let applied = events.apply(&crate::points::payload(
        &crate::points::PointsIntent::SetPoints { points: Vec::new() },
    ));
    assert!(!applied.applied);
    assert!(applied.reason.is_some());
    assert_eq!(events.0, vec![at(3.0)], "and nothing moved");
}
