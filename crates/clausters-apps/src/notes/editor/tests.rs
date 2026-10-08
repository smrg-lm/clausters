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
    let roll = &tree["children"][1];
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

/// **A roll that paints what is played says so to the host**: opened with
/// `midi_in`, or told by a `sync`, the roll carries the prop, and a correction
/// states it either way. What the host paints comes back as `"notes"`, the
/// report a hand's edit makes.
#[test]
fn a_roll_that_paints_what_is_played_carries_midi_in() {
    let roll = |window: &Value| -> Value {
        fn find(node: &Value) -> Option<&Value> {
            if node["type"] == "notes" {
                return Some(node);
            }
            node["children"].as_array()?.iter().find_map(find)
        }
        find(window).cloned().expect("the window holds a roll")
    };
    let mut plain = editor(shared());
    assert!(
        roll(&plain.window(7)).get("midi_in").is_none(),
        "off by default"
    );

    let mut listening = new_json(shared(), r#"{"rate": 100.0, "midi_in": true}"#);
    assert_eq!(roll(&listening.window(7))["midi_in"], 1);
    call_json(
        &mut listening,
        r#"{"verb": "sync", "window": 1, "midi_in": false}"#,
    );
    let corrected: Value = serde_json::from_str(&call_json(
        &mut listening,
        r#"{"verb": "props", "widget": 7}"#,
    ))
    .unwrap();
    assert_eq!(corrected["midi_in"], 0, "a correction states the switch");
    assert!(roll(&listening.window(7)).get("midi_in").is_none());
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
    assert_eq!(out.play, Some(json!({"looping": false, "range": null})));
    assert!(e.window(40)["plays"].as_bool().unwrap());
}

/// **A sweep's time range is what the space bar plays**, in beats, with the
/// loop switch beside it; a range of no length is none.
#[test]
fn the_space_bar_plays_the_time_range_a_sweep_left() {
    let mut e = editor(shared());
    let space = |e: &mut NotesEditor, looping: i64| {
        e.event(
            &Event {
                addr: "/gui_event".into(),
                args: vec![json!(39), json!(3), json!(0), json!("play"), json!(looping)],
            },
            1,
        )
        .play
    };
    e.event(&gesture("selection", vec![json!(50.0), json!(100.0)]), 1);
    assert_eq!(
        space(&mut e, 1),
        Some(json!({"looping": true, "range": [0.5, 1.5]}))
    );
    e.event(&gesture("selection", vec![json!(80.0), json!(0.0)]), 1);
    assert_eq!(
        space(&mut e, 0),
        Some(json!({"looping": false, "range": null}))
    );
}

/// **The time range is one state, the hand's and the client's**: a sweep
/// reports it, the client sets it through `span`, the roll draws it, and the
/// space bar plays it either way.
#[test]
fn a_span_set_by_the_client_is_drawn_and_played_as_a_sweep_is() {
    let mut e = editor(shared());
    let swept = e.event(&gesture("selection", vec![json!(50.0), json!(100.0)]), 1);
    assert_eq!(swept.span, Some(json!([0.5, 1.5])), "a sweep reports it");
    call_json(&mut e, r#"{"verb": "span", "span": [1.0, 2.0]}"#);
    let props: Value =
        serde_json::from_str(&call_json(&mut e, r#"{"verb": "props", "widget": 40}"#)).unwrap();
    assert_eq!(
        (props["sel_start"].clone(), props["sel_len"].clone()),
        (json!(100.0), json!(100.0))
    );
    let space = e.event(
        &Event {
            addr: "/gui_event".into(),
            args: vec![json!(39), json!(3), json!(0), json!("play"), json!(0)],
        },
        1,
    );
    assert_eq!(
        space.play,
        Some(json!({"looping": false, "range": [1.0, 2.0]}))
    );
    call_json(&mut e, r#"{"verb": "span", "span": null}"#);
    let props: Value =
        serde_json::from_str(&call_json(&mut e, r#"{"verb": "props", "widget": 40}"#)).unwrap();
    assert_eq!(props["sel_len"], json!(0.0), "no span, no band");
}

/// **`L` over the window changes the pass in progress**: the loop switch's
/// new state and the time range a sweep left, apart from a play.
#[test]
fn the_loop_key_asks_the_pass_in_progress_to_follow() {
    let mut e = editor(shared());
    e.event(&gesture("selection", vec![json!(50.0), json!(100.0)]), 1);
    let out = e.event(
        &Event {
            addr: "/gui_event".into(),
            args: vec![json!(39), json!(3), json!(0), json!("loop"), json!(1)],
        },
        1,
    );
    assert_eq!(out.turn, Kind::Route);
    assert_eq!(out.play, None, "no play");
    assert_eq!(
        out.relooped,
        Some(json!({"looping": true, "range": [0.5, 1.5]}))
    );
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
    let roll = &e.window(40)["children"][1];
    assert_eq!(roll["axes"]["x"]["playhead_at"], 0.0);
}

/// **A roll in hertz** is told its unit and a window in hertz around its
/// notes, draws each note at its frequency, and a note reported at a new
/// frequency is that frequency -- with the MIDI note it was written with
/// following it, through the pitch family's coherence.
#[test]
fn a_roll_in_hertz_draws_and_edits_frequencies() {
    let sequence = shared();
    let mut e = new_json(
        sequence.clone(),
        r#"{"rate": 100.0, "version": 1, "domain": "hz"}"#,
    );
    let tree = e.window(40);
    call_json(&mut e, r#"{"verb": "sync", "window": 39}"#);
    let roll = &tree["children"][1];
    assert_eq!(roll["axes"]["y"]["unit"], "hz");
    let (min, max) = (
        roll["axes"]["y"]["min"].as_f64().unwrap(),
        roll["axes"]["y"]["max"].as_f64().unwrap(),
    );
    assert!(min < 261.63 && max > 329.63, "{min} {max}");
    let y = roll["notes"][2].as_f64().unwrap();
    assert!((y - 261.6256).abs() < 1e-3, "middle C in hertz: {y}");

    // The first note to 300 Hz, the second as it was.
    let out = e.event(
        &gesture(
            "notes",
            vec![
                json!(1),
                json!(0.0),
                json!(100.0),
                json!(300.0),
                json!(13.0),
                json!(0.0),
                json!(2),
                json!(100.0),
                json!(100.0),
                json!(329.6276),
                json!(51.0),
                json!(0.0),
            ],
        ),
        1,
    );
    assert!(out.changed);
    let held = sequence.lock().unwrap();
    let first = &held.get(1).unwrap().data.0;
    assert!((first["freq"].as_f64().unwrap() - 300.0).abs() < 1e-6);
    let midinote = first["midinote"].as_f64().unwrap();
    assert!((midinote - 62.37).abs() < 0.01, "{midinote}");
    assert_eq!(held.get(2).unwrap().data.0["midinote"], json!(64));
}

/// **A roll navigates its whole domain and opens on its notes**: the compass
/// is MIDI notes 0 to 127 -- or, in hertz, past the highest MIDI note -- and
/// the window the view starts on is the notes with air around them.
#[test]
fn a_roll_navigates_its_whole_domain_and_opens_on_its_notes() {
    let mut midi = editor(shared());
    let y = &midi.window(40)["children"][1]["axes"]["y"];
    assert_eq!(
        (y["min"].as_f64(), y["max"].as_f64()),
        (Some(0.0), Some(127.0))
    );
    let (start, len) = (y["start"].as_f64().unwrap(), y["len"].as_f64().unwrap());
    let (low, high) = (start * 127.0, (start + len) * 127.0);
    assert!(low <= 60.0 && high >= 64.0 && len < 1.0, "{low} {high}");

    let mut hz = new_json(shared(), r#"{"rate": 100.0, "version": 1, "domain": "hz"}"#);
    let y = &hz.window(40)["children"][1]["axes"]["y"];
    assert!(y["max"].as_f64().unwrap() > 12_543.9, "past MIDI note 127");
    let (floor, ceiling) = (
        clausters_core::scale::hz_to_midi(y["min"].as_f64().unwrap()),
        clausters_core::scale::hz_to_midi(y["max"].as_f64().unwrap()),
    );
    let (start, len) = (y["start"].as_f64().unwrap(), y["len"].as_f64().unwrap());
    let low = floor + start * (ceiling - floor);
    let high = low + len * (ceiling - floor);
    assert!(low <= 60.0 && high >= 64.0 && len < 1.0, "{low} {high}");
}

/// **A curve drawn on the roll is one edit of the sequence**: the window draws
/// the curve, a `points` gesture lands on it, and the entry puts it back.
#[test]
fn a_curve_drawn_on_the_roll_is_an_edit_of_the_sequence() {
    use clausters_document::NodeId;
    use clausters_document::multitrack::Automation;
    let sequence = shared();
    let curve = sequence
        .lock()
        .unwrap()
        .edit(EventsIntent::Automation {
            automation: Automation::new(NodeId(0), Opaque(json!({"cc": 1}))),
        })
        .unwrap()
        .added
        .unwrap();
    let mut e = editor(sequence.clone());
    let tree = e.window(40);
    assert_eq!(tree["children"][1]["curves"][0], json!(curve.to_string()));
    let name = json!(curve.to_string());
    let drawn = vec![
        name.clone(),
        json!(0.0),
        json!(10.0),
        json!(1),
        json!(0.0),
        name,
        json!(100.0),
        json!(90.0),
        json!(1),
        json!(0.0),
    ];
    let out = e.event(&gesture("points", drawn), 1);
    assert!(out.changed);
    let record = out.record.expect("an entry");
    assert_eq!(record.label, "draw a curve");
    let points = sequence.lock().unwrap().automation[0].points.clone();
    assert_eq!((points[1].at.0, points[1].value), (1.0, 90.0));
    assert!(e.apply(&record.legs[0].backward));
    assert!(sequence.lock().unwrap().automation[0].points.is_empty());
}

/// **The roll lands on a grid in beats**, a sixteenth until somebody chooses
/// another: the window states it on its time axis, a correction carries it,
/// and the door reads and sets it.
#[test]
fn the_roll_has_a_grid_in_beats() {
    let mut e = editor(shared());
    let tree = e.window(40);
    assert_eq!(
        tree["children"][1]["axes"]["x"]["grid"],
        json!(crate::DEFAULT_GRID)
    );
    assert_eq!(
        call_json(&mut e, r#"{"verb": "grid"}"#),
        r#"{"grid":0.25,"snap":true}"#
    );
    call_json(&mut e, r#"{"verb": "sync", "grid": 1.0}"#);
    assert_eq!(e.grid(), 1.0);
    let props: Value =
        serde_json::from_str(&call_json(&mut e, r#"{"verb": "props", "widget": 40}"#)).unwrap();
    assert_eq!(
        props["grid"],
        json!(1.0),
        "a correction states the grid it lands on"
    );
    call_json(&mut e, r#"{"verb": "sync", "grid": -3.0}"#);
    assert_eq!(e.grid(), 0.0, "no negative grid: none");
}

/// **Snap to grid is a switch, and the window's verb**: on until it is turned
/// off, by the window's G or its View menu, and a correction says which.
#[test]
fn the_snap_to_grid_switch_is_the_window_s_verb() {
    let mut e = editor(shared());
    let tree = e.window(40);
    assert_eq!(tree["children"][1]["axes"]["x"]["grid_snap"], json!(true));
    let window_verb = Event {
        addr: "/gui_event".into(),
        args: vec![json!(39), json!(1), json!(0), json!("snap")],
    };
    e.event(&window_verb, 1);
    assert!(!e.snap(), "off");
    let props: Value =
        serde_json::from_str(&call_json(&mut e, r#"{"verb": "props", "widget": 40}"#)).unwrap();
    assert_eq!(props["grid_snap"], json!(false), "and the roll is told");
    assert_eq!(
        call_json(&mut e, r#"{"verb": "grid"}"#),
        r#"{"grid":0.25,"snap":false}"#
    );
    call_json(&mut e, r#"{"verb": "sync", "snap": true}"#);
    assert!(e.snap());
}
