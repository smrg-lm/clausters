use serde_json::json;

use super::*;
use clausters_document::events::Event as SeqEvent;

fn shared() -> Shared {
    Arc::new(Mutex::new(EventSequence::new(vec![
        SeqEvent::new(0.0, json!({"midinote": 60, "sustain": 1.0, "amp": 0.1})),
        SeqEvent::new(1.0, json!({"midinote": 64, "sustain": 1.0, "amp": 0.4})),
    ])))
}

fn editor(sequence: Shared) -> NotesEditor {
    let mut editor = new_json(sequence, r#"{"rate": 100.0, "version": 1}"#);
    editor.window(40);
    call_json(&mut editor, r#"{"verb": "sync", "window": 39}"#);
    editor
}

fn gesture(tag: &str, values: Vec<Value>) -> Event {
    let mut args = vec![json!(40), json!(1), json!(0), json!(tag)];
    args.extend(values);
    Event {
        addr: "/gui_event".into(),
        args,
    }
}

#[test]
fn the_window_draws_the_notes_with_their_ids() {
    let mut e = editor(shared());
    let tree = e.window(40);
    let roll = &tree["children"][0];
    assert_eq!(roll["type"], "notes");
    assert_eq!(roll["id"], 40);
    assert_eq!(roll["note_ids"], json!([1, 2]));
    assert_eq!(
        roll["notes"].as_array().unwrap()[0..5],
        json!([0.0, 100.0, 60.0, 13.0, 0.0]).as_array().unwrap()[..]
    );
}

/// The sequence is edited in place: the holder reads the edit through its own
/// handle, and the entry puts it back.
#[test]
fn a_gesture_edits_the_shared_sequence_and_leaves_its_inverse() {
    let sequence = shared();
    let mut e = editor(sequence.clone());
    let moved = vec![
        json!(1),
        json!(0.0),
        json!(100.0),
        json!(62.0),
        json!(13.0),
        json!(0.0),
        json!(2),
        json!(100.0),
        json!(100.0),
        json!(64.0),
        json!(51.0),
        json!(0.0),
    ];
    let out = e.event(&gesture("notes", moved), 1);
    assert_eq!(out.turn, Kind::Route);
    assert!(out.changed && out.version == 2);
    assert_eq!(
        sequence.lock().unwrap().get(1).unwrap().data.0["midinote"],
        json!(62.0)
    );
    let record = out.record.expect("an entry");
    assert_eq!(record.label, "edit the notes");
    // The entry's inverse puts the sequence back.
    let backward = record.legs[0].backward.clone();
    assert!(e.apply(&backward));
    assert_eq!(
        sequence.lock().unwrap().get(1).unwrap().data.0["midinote"],
        json!(60)
    );
    // The answer corrects the roll with the notes and their ids.
    let Some(Answer::Push { corrections, .. }) = out.answer else {
        panic!("an acknowledgement with corrections");
    };
    assert_eq!(corrections[0].props["note_ids"], json!([1, 2]));
}

#[test]
fn a_roll_that_cannot_be_edited_refuses_and_says_why() {
    let mut e = new_json(shared(), r#"{"rate": 100.0, "editable": false}"#);
    e.window(40);
    let out = e.event(&gesture("notes", vec![]), 1);
    assert!(!out.changed);
    let Some(Answer::Push { reason, .. }) = out.answer else {
        panic!("an acknowledgement with corrections");
    };
    assert!(reason.unwrap().contains("rendering"));
}

#[test]
fn the_space_bar_over_the_window_asks_for_a_play() {
    let mut e = editor(shared());
    let out = e.event(
        &Event {
            addr: "/gui_event".into(),
            args: vec![json!(39), json!(3), json!(0), json!("play"), json!(0)],
        },
        1,
    );
    assert_eq!(out.turn, Kind::Route);
    assert_eq!(out.play, Some(json!({"looping": false})));
    assert!(e.window(40)["plays"].as_bool().unwrap());
}

/// **A click on the ruler places the position cursor**, as a beat of the
/// sequence, and edits nothing; the roll's play cursor is anchored so the
/// transport's position is where it is drawn.
#[test]
fn a_locate_places_the_cursor_on_a_beat_and_edits_nothing() {
    let mut e = editor(shared());
    let out = e.event(&gesture("locate", vec![json!(150.0)]), 1);
    // 100 samples a beat: one beat a second at a rate of 100.
    assert_eq!(out.locate, Some(1.5));
    assert!(!out.changed);
    assert!(out.record.is_none());
    let roll = &e.window(40)["children"][0];
    assert_eq!(roll["axes"]["x"]["playhead_at"], 0.0);
}
