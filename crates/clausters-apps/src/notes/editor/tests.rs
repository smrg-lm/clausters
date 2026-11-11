use serde_json::json;

use super::*;
use clausters_document::events::Event as SeqEvent;

fn shared() -> Shared {
    Arc::new(Mutex::new(EventSequence::new(vec![
        SeqEvent::new(0.0, json!({"midinote": 60, "sustain": 1.0, "amp": 0.1})),
        SeqEvent::new(1.0, json!({"midinote": 64, "sustain": 1.0, "amp": 0.4})),
    ])))
}

/// [`shared`]'s two notes written as a page, read as they were played.
#[cfg(feature = "notation")]
fn page() -> Shared {
    let written = shared().lock().unwrap().notated(None, None, None).unwrap();
    Arc::new(Mutex::new(written))
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

/// **The window's loop tool reads the switch**, which the host keeps and tells
/// the window of as it turns.
#[test]
fn the_loop_switch_is_read_where_a_tool_reads_it() {
    let mut e = editor(shared());
    assert_eq!(e.actions(false, false)["loop"].checked, Some(false));
    let turned = |on: i64| Event {
        addr: "/gui_event".into(),
        args: vec![json!(39), json!(3), json!(0), json!("loop"), json!(on)],
    };
    e.event(&turned(1), 1);
    assert_eq!(e.actions(false, false)["loop"].checked, Some(true));
    e.event(&turned(0), 1);
    assert_eq!(e.actions(false, false)["loop"].checked, Some(false));
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

/// **The roll's window is the reader's**, as the multitrack's is: its axis
/// does not follow a change of what it holds, so entering notation mode,
/// which draws the last note at its written value and moves where the
/// notes end, does not page the view forward onto that end.
#[test]
fn the_rolls_window_does_not_follow_its_content() {
    let mut e = editor(shared());
    let roll = &e.window(40)["children"][1];
    assert_eq!(roll["autofit"], json!(false));
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

/// A note, an id and a marker in one sequence, the ids its events take.
fn with_markers(markers: &[(f64, &str, i64)]) -> Shared {
    let mut events = vec![SeqEvent::new(
        0.0,
        json!({"midinote": 60, "sustain": 1.0, "instrument": "bell"}),
    )];
    events.extend(markers.iter().map(|(at, addr, value)| {
        SeqEvent::new(*at, json!({"type": "osc", "addr": addr, "args": [value]}))
    }));
    Arc::new(Mutex::new(EventSequence::new(events)))
}

/// What the roll cannot draw stays on the note it moves, and on the events
/// it does not draw.
#[test]
fn a_note_keeps_what_the_roll_cannot_draw() {
    let sequence = with_markers(&[(3.0, "/mark", 1)]);
    let mut e = editor(sequence.clone());
    let notes = vec![
        json!(1),
        json!(0.0),
        json!(100.0),
        json!(65.0),
        json!(13.0),
        json!(0.0),
    ];
    assert!(e.event(&gesture("notes", notes), 1).changed);
    let held = sequence.lock().unwrap();
    let note = held.get(1).unwrap();
    assert_eq!(note.data.0["midinote"], json!(65.0));
    assert_eq!(note.data.0["instrument"], "bell");
    assert_eq!(
        held.get(2).unwrap().data.0["type"],
        "osc",
        "the marker stays"
    );
}

/// A note is named by its id, never by its place: the one left out is the
/// one removed, and one with no id is minted one, which the roll is told.
#[test]
fn a_note_is_named_by_its_id_and_a_new_one_is_told_its_own() {
    let sequence = shared();
    let mut e = editor(sequence.clone());
    let out = e.event(
        &gesture(
            "notes",
            vec![
                json!(2),
                json!(200.0),
                json!(100.0),
                json!(65.0),
                json!(13.0),
                json!(0.0),
            ],
        ),
        1,
    );
    assert!(out.changed);
    {
        let held = sequence.lock().unwrap();
        assert!(held.get(1).is_none(), "the one left out is the one removed");
        assert_eq!(held.get(2).unwrap().at.0, 2.0, "and the other keeps its id");
    }
    let out = e.event(
        &gesture(
            "notes",
            vec![
                json!(2),
                json!(200.0),
                json!(100.0),
                json!(65.0),
                json!(13.0),
                json!(0.0),
                json!(0),
                json!(300.0),
                json!(100.0),
                json!(67.0),
                json!(90.0),
                json!(0.0),
            ],
        ),
        out.version,
    );
    assert!(out.changed);
    let told = serde_json::to_value(&out.answer).unwrap().to_string();
    assert!(
        told.contains("note_ids"),
        "the roll is told the new id: {told}"
    );
    assert_eq!(sequence.lock().unwrap().events.len(), 2);
}

/// A marker is matched by the message it sends: a drag moves it, what it sends
/// stays, and leaving one out removes that one and no other.
#[test]
fn a_marker_is_moved_and_removed_by_the_message_it_sends() {
    let sequence = with_markers(&[(0.0, "/a", 1), (1.0, "/b", 2), (2.0, "/c", 3)]);
    let mut e = editor(sequence.clone());
    let out = e.event(
        &gesture(
            "osc",
            vec![
                json!(50.0),
                json!("/a"),
                json!(100.0),
                json!("/b"),
                json!(200.0),
                json!("/c"),
            ],
        ),
        1,
    );
    assert!(out.changed);
    assert_eq!(out.record.as_ref().unwrap().label, "edit the markers");
    let markers = |sequence: &Shared| -> Vec<(f64, Value)> {
        sequence
            .lock()
            .unwrap()
            .events
            .iter()
            .filter(|event| event.data.0["type"] == "osc")
            .map(|event| (event.at.0, event.data.0["addr"].clone()))
            .collect()
    };
    assert_eq!(markers(&sequence)[0], (0.5, json!("/a")));
    assert_eq!(
        sequence.lock().unwrap().get(2).unwrap().data.0["args"],
        json!([1]),
        "the message it sends is not the lane's to lose"
    );
    let out = e.event(
        &gesture(
            "osc",
            vec![json!(50.0), json!("/a"), json!(200.0), json!("/c")],
        ),
        out.version,
    );
    assert!(out.changed);
    assert_eq!(
        markers(&sequence),
        vec![(0.5, json!("/a")), (2.0, json!("/c"))]
    );
}

/// A marker is the message it sends, and the lane has no way to type one: one
/// added there is refused, says why, and the lane is put back.
#[test]
fn a_marker_added_in_the_roll_is_refused_and_says_why() {
    let sequence = with_markers(&[(0.0, "/a", 1)]);
    let mut e = editor(sequence.clone());
    let out = e.event(
        &gesture(
            "osc",
            vec![json!(0.0), json!("/a"), json!(100.0), json!("")],
        ),
        1,
    );
    assert!(!out.changed);
    assert_eq!(
        sequence.lock().unwrap().events.len(),
        2,
        "nothing was added"
    );
    let told = serde_json::to_value(&out.answer).unwrap().to_string();
    assert!(told.contains("a marker is the message it sends"), "{told}");
    assert!(told.contains("\"osc\""), "the lane is put back: {told}");
}

/// **The monophonic switch is the window's**, as snap is: M flips it, nothing
/// is recorded, the menu reads it; and a gesture then shortens what it
/// covers in one entry.
#[test]
fn the_monophonic_switch_is_the_window_s_and_a_gesture_obeys_it() {
    let sequence = shared();
    let mut e = editor(sequence.clone());
    let tree = e.window(40);
    assert_eq!(tree["keys"], json!(["notes"]));
    assert_eq!(tree["verbs"]["notes"]["monophonic"]["keys"], json!(["M"]));
    let out = e.event(
        &Event {
            addr: "/gui_event".into(),
            args: vec![json!(39), json!(1), json!(0), json!("monophonic")],
        },
        1,
    );
    assert!(out.record.is_none() && e.editing().monophonic);
    assert_eq!(e.actions(false, false)["monophonic"].checked, Some(true));
    let editing: Value =
        serde_json::from_str(&call_json(&mut e, r#"{"verb": "editing"}"#)).unwrap();
    assert_eq!(editing["monophonic"], json!(true));
    assert_eq!(editing["mode"], json!("midi"));
    // the first note lengthened over the second: the second starts where
    // the first now ends
    let values = vec![
        json!(1),
        json!(0.0),
        json!(150.0),
        json!(60.0),
        json!(13.0),
        json!(0.0),
        json!(2),
        json!(100.0),
        json!(100.0),
        json!(64.0),
        json!(51.0),
        json!(0.0),
    ];
    let out = e.event(&gesture("notes", values), 1);
    assert!(out.record.is_some());
    assert_eq!(sequence.lock().unwrap().get(2).unwrap().at.0, 1.5);
}

/// **`join` and `unjoin` from the roll are the sequence's edits**, recorded.
#[test]
fn a_join_from_the_roll_is_an_edit_of_the_sequence() {
    let sequence = shared();
    let mut e = editor(sequence.clone());
    let out = e.event(&gesture("join", vec![json!(1), json!(2)]), 1);
    assert_eq!(out.record.expect("an entry").label, "join");
    assert_eq!(
        sequence.lock().unwrap().get(2).unwrap().data.0["joins"],
        json!(1)
    );
    let out = e.event(&gesture("unjoin", vec![json!(2)]), 2);
    assert!(out.record.is_some());
    assert!(
        sequence
            .lock()
            .unwrap()
            .get(2)
            .unwrap()
            .data
            .0
            .get("joins")
            .is_none()
    );
}

/// **In MIDI a join from the roll holds one pitch on**: MIDI has no join,
/// so two notes of one pitch become one, and notes of different pitches are
/// refused, the sequence left as it was.
#[test]
fn a_join_from_the_roll_in_midi_holds_one_pitch_on() {
    let mut midi = EventSequence::new(vec![
        SeqEvent::new(0.0, json!({"midinote": 60, "sustain": 1.0})),
        SeqEvent::new(1.0, json!({"midinote": 64, "sustain": 1.0})),
        SeqEvent::new(2.0, json!({"midinote": 60, "sustain": 1.0})),
    ]);
    midi.midi = Some(clausters_document::events::MidiSpec::Midi1);
    let sequence = Arc::new(Mutex::new(midi));
    let mut e = editor(sequence.clone());
    let before = sequence.lock().unwrap().clone();
    let out = e.event(&gesture("join", vec![json!(1), json!(2)]), 1);
    assert!(out.record.is_none(), "two pitches are not joined");
    assert_eq!(*sequence.lock().unwrap(), before);
    let out = e.event(&gesture("join", vec![json!(1), json!(3)]), 2);
    assert_eq!(out.record.expect("an entry").label, "join");
    let held = sequence.lock().unwrap();
    let ids: Vec<u64> = held.events.iter().map(|n| n.id).collect();
    assert_eq!(ids, vec![1, 2], "the second 60 is part of the first");
    assert!((held.get(1).unwrap().sustain() - 3.0).abs() < 1e-9);
}

/// **A split from the roll is the document's**: the cut, in the axis's
/// units, and the notes; one entry, and the second half a new event with the
/// first's keys.
#[test]
fn a_split_from_the_roll_is_an_edit_of_the_sequence() {
    let sequence = shared();
    let mut e = editor(sequence.clone());
    // 100 units a beat: the first note cut halfway
    let out = e.event(&gesture("split", vec![json!(50.0), json!(1)]), 1);
    assert_eq!(out.record.expect("an entry").label, "split");
    let held = sequence.lock().unwrap();
    assert_eq!(held.events.len(), 3);
    let second = held.events.iter().find(|n| n.at.0 == 0.5).expect("a half");
    assert_eq!(second.data.0["amp"], json!(0.1));
    assert!((held.get(1).unwrap().sustain() - 0.5).abs() < 1e-9);
    drop(held);
    let out = e.event(&gesture("split", vec![json!(500.0), json!(1)]), 2);
    assert!(out.record.is_none(), "a cut outside the note edits nothing");
}

/// **A selection by data corrects the roll's selection** and edits nothing.
#[test]
fn a_selection_by_pitch_is_the_window_s_and_records_nothing() {
    let sequence = Arc::new(Mutex::new(EventSequence::new(vec![
        SeqEvent::new(0.0, json!({"midinote": 60, "sustain": 1.0})),
        SeqEvent::new(1.0, json!({"midinote": 64, "sustain": 1.0})),
        SeqEvent::new(2.0, json!({"midinote": 60, "sustain": 1.0})),
    ])));
    let mut e = editor(sequence);
    let out = e.event(&gesture("select_pitch", vec![json!(1)]), 1);
    assert!(out.record.is_none());
    let told = serde_json::to_string(&out.answer).unwrap();
    assert!(told.contains("\"selected\":[1,3]"), "{told}");
}

/// **What the notation mode shows and the voice it edits are the
/// window's**: a staff, a part or the system shown, the staff edited moved
/// up and down and refused past the edge, the voice chosen; none of it is
/// recorded, and over a performance each is refused with why.
#[cfg(feature = "notation")]
#[test]
fn the_notation_mode_shows_a_staff_and_edits_a_voice_of_it() {
    let sequence = Arc::new(Mutex::new(EventSequence::new(vec![
        SeqEvent::new(
            0.0,
            json!({"midinote": 72, "dur": 1.0, "value": [1, 4], "staff": 0, "voice": 0}),
        ),
        SeqEvent::new(
            0.0,
            json!({"midinote": 48, "dur": 1.0, "value": [1, 4], "staff": 1, "voice": 0}),
        ),
    ])));
    let mut midi = editor(shared());
    let out = midi.event(&window_verb("show_staff"), 1);
    assert!(
        out.record.is_none() && midi.editing().scope == Scope::System,
        "MIDI mode refuses"
    );
    let mut e = editor(sequence);
    assert_eq!(e.editing().mode, Mode::Notation, "the format's mode");
    let out = e.event(&window_verb("staff_up"), 2);
    let told = serde_json::to_string(&out.answer).unwrap();
    assert!(told.contains("top staff"), "{told}");
    e.event(&window_verb("show_staff"), 2);
    e.event(&window_verb("staff_down"), 2);
    assert_eq!(e.editing().voice, [1, 0]);
    assert_eq!(
        e.editing().scope,
        Scope::Staff(1),
        "the staff shown follows"
    );
    let out = e.event(&window_verb("staff_down"), 2);
    let told = serde_json::to_string(&out.answer).unwrap();
    assert!(told.contains("bottom staff"), "{told}");
    e.event(&window_verb("voice_2"), 2);
    assert_eq!(e.editing().voice, [1, 1]);
    e.event(&window_verb("show_system"), 2);
    assert_eq!(e.editing().scope, Scope::System);
    assert!(out.record.is_none());
}

/// **The panel follows the roll and edits through it**: shown by its verb
/// (the window drawn again), the roll's marks shown in the inspector and the
/// list, a velocity typed written on the marked note as one entry, and a row
/// chosen in the list marked on the roll.
#[test]
fn the_panel_follows_the_roll_and_edits_through_it() {
    let sequence = shared();
    let mut e = new_json(sequence.clone(), r#"{"rate": 100.0, "version": 1}"#);
    let names: Value = serde_json::from_str(&call_json(&mut e, r#"{"verb": "dialogs"}"#)).unwrap();
    let numbered: serde_json::Map<String, Value> = names["dialogs"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str().unwrap().to_string(), json!(200 + i as i64)))
        .collect();
    let ids = |name: &str| numbered[name].as_i64().unwrap();
    let request = json!({"verb": "window", "widget": 40, "dialogs": numbered}).to_string();
    let tree = call_json(&mut e, &request);
    assert!(!tree.contains("\"table\""), "hidden at first");
    call_json(&mut e, r#"{"verb": "sync", "window": 39}"#);
    let out = e.event(&window_verb("events_panel"), 1);
    assert!(out.restructure && out.record.is_none());
    let tree = e.window(40).to_string();
    assert!(
        tree.contains("\"table\"") && tree.contains("tell_marks"),
        "{tree}"
    );
    // the roll says what the hand marked
    let out = e.event(&gesture("marked", vec![json!(1)]), 1);
    let told = serde_json::to_string(&out.answer).unwrap();
    assert!(told.contains("\"Velocity\""), "{told}");
    // a velocity typed in the inspector
    let field = Event {
        addr: "/gui_event".into(),
        args: vec![
            json!(ids("inspect:velocity")),
            json!(2),
            json!(1),
            json!(110.0),
        ],
    };
    let out = e.event(&field, 1);
    assert!(out.record.is_some(), "one entry");
    let keys = sequence.lock().unwrap().get(1).unwrap().keys();
    assert_eq!(keys["velocity"], json!(110.0));
    // a row chosen in the list marks its note
    let list = Event {
        addr: "/gui_event".into(),
        args: vec![
            json!(ids("events:list")),
            json!(3),
            json!(2),
            json!("select"),
            json!(1),
        ],
    };
    let out = e.event(&list, 2);
    let told = serde_json::to_string(&out.answer).unwrap();
    assert!(told.contains("\"selected\":[2]"), "{told}");
}

fn window_verb(verb: &str) -> Event {
    Event {
        addr: "/gui_event".into(),
        args: vec![json!(39), json!(1), json!(0), json!(verb)],
    }
}

/// **Converting a take to notation makes a copy**, which a roll of its own
/// opens: the take and its roll are as they were, and nothing is recorded.
/// The copy is in notation mode, and draws what is written: each note's
/// voice, how long it sounds inside how long it is written. Converting it
/// to notation again is refused.
#[cfg(feature = "notation")]
#[test]
fn converting_a_take_to_notation_opens_a_copy_and_leaves_the_take() {
    let sequence = Arc::new(Mutex::new(EventSequence::new(vec![
        SeqEvent::new(
            0.03,
            json!({"midinote": 60, "dur": 0.97, "instrument": "bell"}),
        ),
        SeqEvent::new(
            1.0,
            json!({"midinote": 64, "dur": 1.0, "articulations": ["stacc"]}),
        ),
    ])));
    let before = sequence.lock().unwrap().clone();
    let mut e = editor(sequence.clone());
    let out = e.event(&window_verb("convert_notation"), 1);
    assert!(out.record.is_none(), "a copy is no edit");
    assert_eq!(*sequence.lock().unwrap(), before, "the take is unchanged");
    assert_eq!(e.editing().mode, Mode::Midi);
    let converted = out.converted.expect("the copy");
    assert_eq!(converted["title"], json!("Notes (notation)"));
    let copy: EventSequence = serde_json::from_value(converted["sequence"].clone()).unwrap();
    assert!(copy.written_as_notation());
    assert_eq!(copy.events[0].at.0, 0.0, "where it is written");
    assert_eq!(copy.events[0].data.0["instrument"], json!("bell"));
    let mut page = editor(Arc::new(Mutex::new(copy)));
    assert_eq!(page.editing().mode, Mode::Notation);
    let props: Value =
        serde_json::from_str(&call_json(&mut page, r#"{"verb": "props", "widget": 40}"#)).unwrap();
    assert_eq!(props["voices"], json!([0, 0, 0, 0]));
    let held: Vec<f64> = serde_json::from_value(props["held"].clone()).unwrap();
    let notes: Vec<f64> = serde_json::from_value(props["notes"].clone()).unwrap();
    assert!(
        held[1] < notes[6],
        "the staccato sounds shorter than it is written"
    );
    let out = page.event(&window_verb("convert_notation"), 2);
    assert!(out.converted.is_none());
    let told = serde_json::to_string(&out.answer).unwrap();
    assert!(told.contains("already"), "{told}");
    assert!(!page.actions(false, false)["convert_notation"].enabled);
}

/// **A page converted to MIDI is a copy that sounds as the page did**, its
/// notation left out, and the page is as it was.
#[cfg(feature = "notation")]
#[test]
fn converting_a_page_to_midi_opens_a_copy_and_leaves_the_page() {
    let sequence = page();
    let before = sequence.lock().unwrap().clone();
    let mut e = editor(sequence.clone());
    let out = e.event(&window_verb("convert_midi2"), 1);
    assert!(out.record.is_none());
    assert_eq!(*sequence.lock().unwrap(), before);
    let converted = out.converted.expect("the copy");
    assert_eq!(converted["title"], json!("Notes (MIDI 2.0)"));
    let copy: EventSequence = serde_json::from_value(converted["sequence"].clone()).unwrap();
    assert_eq!(copy.format().name(), "2.0");
    assert!(
        copy.events
            .iter()
            .all(|ev| ev.data.0.get("value").is_none())
    );
}

/// **A window with the dialog asks how before it converts**: the
/// conversion opens the form holding the stored reading, `Cancel` makes
/// nothing, and `OK` makes the copy with what was chosen; the take is never
/// changed.
#[cfg(feature = "notation")]
#[test]
fn entering_notation_in_a_window_with_the_dialog_asks_how_first() {
    let sequence = shared();
    let mut e = new_json(sequence.clone(), r#"{"rate": 100.0, "version": 1}"#);
    let names: Value = serde_json::from_str(&call_json(&mut e, r#"{"verb": "dialogs"}"#)).unwrap();
    let ids: serde_json::Map<String, Value> = names["dialogs"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, name)| (name.as_str().unwrap().to_string(), json!(100 + i)))
        .collect();
    let tree: Value = serde_json::from_str(&call_json(
        &mut e,
        &json!({"verb": "window", "widget": 40, "dialogs": ids}).to_string(),
    ))
    .unwrap();
    assert!(tree.to_string().contains("\"Write as notation\""));
    call_json(&mut e, r#"{"verb": "sync", "window": 39}"#);
    let said = |e: &mut NotesEditor, name: &str, tag: &str| {
        let event = Event {
            addr: "/gui_event".into(),
            args: vec![ids[name].clone(), json!(1), json!(0), json!(tag)],
        };
        e.event(&event, 1)
    };
    let stack = ids["stack"].as_i64().unwrap();
    let out = e.event(&window_verb("convert_notation"), 1);
    assert!(out.converted.is_none(), "nothing is converted yet");
    let answer = serde_json::to_value(out.answer.unwrap()).unwrap();
    assert!(answer.to_string().contains(&format!("{stack}")));
    // Cancel: nothing is made
    let out = said(&mut e, "notation:cancel", "click");
    assert!(out.converted.is_none() && out.record.is_none());
    // OK, in 3/4 with ties
    e.event(&window_verb("convert_notation"), 1);
    said(&mut e, "notation:meter", "3/4");
    said(&mut e, "notation:writing", "2");
    let out = said(&mut e, "notation:ok", "click");
    assert!(out.record.is_none(), "a copy is no edit");
    assert!(!sequence.lock().unwrap().written_as_notation());
    assert_eq!(e.editing().mode, Mode::Midi);
    let copy: EventSequence =
        serde_json::from_value(out.converted.expect("the copy")["sequence"].clone()).unwrap();
    assert!(copy.written_as_notation());
    assert_eq!(
        copy.parts_read()[0].writing.polyphony,
        clausters_document::events::Polyphony::Ties
    );
    let bars = clausters_editing::notes::bars(&copy);
    assert_eq!(bars[1], 3.0, "a bar of three beats");
}

/// **A meter that is not one keeps the dialog up**, and says why.
#[cfg(feature = "notation")]
#[test]
fn a_meter_that_is_not_one_is_refused_in_the_dialog() {
    let mut e = new_json(shared(), r#"{"rate": 100.0, "version": 1}"#);
    let ids: serde_json::Map<String, Value> = super::dialogs::names()
        .into_iter()
        .enumerate()
        .map(|(i, name)| (name, json!(100 + i)))
        .collect();
    call_json(
        &mut e,
        &json!({"verb": "window", "widget": 40, "dialogs": ids}).to_string(),
    );
    call_json(&mut e, r#"{"verb": "sync", "window": 39}"#);
    e.event(&window_verb("convert_notation"), 1);
    let event = |name: &str, tag: &str| Event {
        addr: "/gui_event".into(),
        args: vec![ids[name].clone(), json!(1), json!(0), json!(tag)],
    };
    e.event(&event("notation:meter", "three"), 1);
    let out = e.event(&event("notation:ok", "click"), 1);
    assert!(out.record.is_none());
    let answer = serde_json::to_value(out.answer.unwrap()).unwrap();
    assert!(answer.to_string().contains("a meter is written as 3/4"));
    assert!(out.converted.is_none());
}

/// **A bare window has no dialog**, and converts as it did.
#[test]
fn a_bare_roll_names_no_dialog() {
    let mut e = new_json(shared(), r#"{"rate": 100.0, "chrome": false}"#);
    assert_eq!(
        call_json(&mut e, r#"{"verb": "dialogs"}"#),
        r#"{"dialogs":[]}"#
    );
}

/// **In notation mode a note lands on a written value, in the voice edited,
/// and sounds as written**; a slur over a selection is a page's mark, heard
/// at once; and the writing of the polyphony is an edit of the parts.
#[cfg(feature = "notation")]
#[test]
fn a_note_in_notation_mode_is_written_and_its_sound_derived() {
    let sequence = page();
    let mut e = editor(sequence.clone());
    e.event(&window_verb("voice_2"), 2);
    assert_eq!(e.editing().voice, [0, 1]);
    // a new note, a little off the grid and a little short of a half
    let props: Value =
        serde_json::from_str(&call_json(&mut e, r#"{"verb": "props", "widget": 40}"#)).unwrap();
    let mut values = Vec::new();
    let notes: Vec<f64> = serde_json::from_value(props["notes"].clone()).unwrap();
    let ids: Vec<u64> = serde_json::from_value(props["note_ids"].clone()).unwrap();
    for (i, id) in ids.iter().enumerate() {
        values.push(json!(id));
        values.extend(notes[i * 5..i * 5 + 5].iter().map(|v| json!(v)));
    }
    values.extend([
        json!(0),
        json!(203.0),
        json!(190.0),
        json!(67.0),
        json!(64.0),
        json!(0.0),
    ]);
    let out = e.event(&gesture("notes", values), 2);
    assert!(out.record.is_some(), "{:?}", out.answer);
    let held = sequence.lock().unwrap().clone();
    let new = held
        .events
        .iter()
        .find(|ev| ev.data.0["midinote"] == json!(67.0))
        .unwrap();
    assert_eq!(new.at.0, 2.0, "on the grid");
    assert_eq!(new.data.0["value"], json!([1, 2]), "a half");
    assert_eq!(
        (new.data.0["staff"].clone(), new.data.0["voice"].clone()),
        (json!(0), json!(1))
    );
    assert!(
        new.data.0.get("sustain").is_some(),
        "what it sounds is derived"
    );
    // a slur over the first two
    let first: Vec<u64> = held.events.iter().take(2).map(|ev| ev.id).collect();
    let out = e.event(&gesture("slur", vec![json!(first[0]), json!(first[1])]), 3);
    assert_eq!(out.record.expect("an entry").label, "write a slur");
    let spanners = sequence.lock().unwrap().notation.clone().unwrap()["spanners"].clone();
    assert!(
        spanners
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["kind"] == "slur")
    );
    // the writing
    let out = e.event(&window_verb("write_ties"), 4);
    assert_eq!(
        out.record.expect("an entry").label,
        "write the polyphony with ties"
    );
    assert_eq!(e.actions(true, false)["write_ties"].checked, Some(true));
}

/// **A mark of a page is the notation mode's**: in MIDI mode it is refused
/// with why, and the menu reads it unavailable.
#[test]
fn a_page_s_mark_waits_for_the_notation_mode() {
    let mut e = editor(shared());
    let out = e.event(&gesture("slur", vec![json!(1), json!(2)]), 1);
    assert!(out.record.is_none());
    assert!(!e.actions(false, false)["slur"].enabled);
}

/// **Without the notation layer a conversion into notation says why it is
/// not there**, and makes nothing.
#[cfg(not(feature = "notation"))]
#[test]
fn without_the_notation_layer_the_conversion_says_why() {
    let mut e = editor(shared());
    let out = e.event(&window_verb("convert_notation"), 1);
    assert!(out.record.is_none() && out.converted.is_none());
    assert_eq!(e.editing().mode, Mode::Midi);
}

/// **A page's marks are drawn with the engraver's symbols**, asked once and
/// only when the roll first shows a page: an accent the engraver has an
/// outline for is its character, and a tenuto it has none for the text the
/// palette labels it with, one mark after another with a space between, for
/// the roll to lay each out. The outlines reach the roll as its `glyphs`.
#[cfg(feature = "notation")]
#[test]
fn a_page_s_marks_are_drawn_with_the_engraver_s_symbols() {
    let mut midi = editor(shared());
    let asked = Arc::new(Mutex::new(0));
    let count = Arc::clone(&asked);
    let outlines: crate::notes::OutlineSource = Arc::new(move |codes: &[&str]| {
        *count.lock().unwrap() += 1;
        codes
            .iter()
            .filter(|c| **c == "E4A0")
            .map(|c| (c.to_string(), "M0 0 L100 0 L100 100 Z".to_string()))
            .collect()
    });
    midi.set_outlines(Some(outlines.clone()));
    midi.event(&gesture("accent", vec![json!(1)]), 1);
    assert_eq!(
        *asked.lock().unwrap(),
        0,
        "a roll in MIDI mode asks nothing"
    );
    let sequence = page();
    let mut e = new_json(sequence.clone(), r#"{"rate": 100.0, "version": 1}"#);
    e.set_outlines(Some(outlines));
    let told = e.window(40).to_string();
    call_json(&mut e, r#"{"verb": "sync", "window": 39}"#);
    assert!(told.contains("\"glyphs\":{\"E4A0\""), "{told}");
    let first = sequence.lock().unwrap().events[0].id;
    e.event(&gesture("accent", vec![json!(first)]), 2);
    let out = e.event(&gesture("tenuto", vec![json!(first)]), 3);
    let Some(Answer::Push { corrections, .. }) = out.answer else {
        panic!("a correction");
    };
    let marks = corrections
        .iter()
        .find_map(|c| c.props.get("marks"))
        .expect("the marks");
    assert_eq!(marks[0], json!("\u{E4A0} -"));
    e.window(40);
    assert_eq!(*asked.lock().unwrap(), 1, "asked once");
}

/// **The lanes are chosen by the tabs the roll draws over its lane**: the
/// velocities first, then each curve of the sequence; a curve added from
/// the roll is one entry and a tab more, and a tab pressed shows its lane in
/// the roll, recording nothing. A roll alone draws its rows stacked, and no
/// tabs.
#[test]
fn the_lanes_are_chosen_by_their_tabs_and_a_curve_added_is_one_entry() {
    let sequence = shared();
    let mut e = new_json(sequence.clone(), r#"{"rate": 100.0, "version": 1}"#);
    let alone = e.window(40);
    assert!(
        alone["children"][1].get("lane_tabs").is_none(),
        "a roll alone"
    );
    let window: Value = serde_json::from_str(&call_json(
        &mut e,
        r#"{"verb": "window", "widget": 40, "dialogs": {"events:list": 41}}"#,
    ))
    .unwrap();
    call_json(&mut e, r#"{"verb": "sync", "window": 39}"#);
    let roll = &window["children"][1];
    assert_eq!(roll["lane_tabs"], json!(["velocity", "velocity"]));
    assert_eq!(roll["lane"], "velocity");
    assert!(
        !window["children"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["view"] == "tabs"),
        "no tabs at the window's foot"
    );
    let out = e.event(&gesture("curve_cc1", vec![]), 1);
    assert!(out.record.is_some(), "one entry");
    let curve = sequence.lock().unwrap().automation[0].id.0;
    let told = serde_json::to_string(&out.answer).unwrap();
    assert!(
        told.contains(&format!(
            "\"lane_tabs\":[\"velocity\",\"velocity\",\"{curve}\",\"CC 1 ch 1\"]"
        )),
        "{told}"
    );
    let out = e.event(&gesture("lane", vec![json!(curve.to_string())]), 2);
    assert!(out.record.is_none());
    let told = serde_json::to_string(&out.answer).unwrap();
    assert!(told.contains(&format!("\"lane\":\"{curve}\"")), "{told}");
}

/// **The display panel says how the roll draws what is beyond its boxes**:
/// shown by its switch (the window drawn again), a report of it is a setting
/// of the window's and no entry, the roll drawn as it says -- the pitch line
/// taken off, the slurs drawn as chosen -- and a lane taken off the tabs.
#[test]
fn the_display_panel_sets_how_the_roll_draws_and_records_nothing() {
    let sequence = shared();
    let mut e = new_json(sequence.clone(), r#"{"rate": 100.0, "version": 1}"#);
    let names: Value = serde_json::from_str(&call_json(&mut e, r#"{"verb": "dialogs"}"#)).unwrap();
    let numbered: serde_json::Map<String, Value> = names["dialogs"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str().unwrap().to_string(), json!(200 + i as i64)))
        .collect();
    let ids = |name: &str| numbered[name].as_i64().unwrap();
    let request = json!({"verb": "window", "widget": 40, "dialogs": numbered}).to_string();
    let tree = call_json(&mut e, &request);
    assert!(
        tree.contains("\"slur_style\":\"arc\""),
        "drawn as the default"
    );
    assert!(
        !tree.contains("Pitch line between notes"),
        "hidden at first"
    );
    call_json(&mut e, r#"{"verb": "sync", "window": 39}"#);
    let out = e.event(&window_verb("display_settings"), 1);
    assert!(out.restructure && out.record.is_none());
    let tree = e.window(40).to_string();
    assert!(tree.contains("Pitch line between notes"), "{tree}");
    let report = |id: i64, value: Value| Event {
        addr: "/gui_event".into(),
        args: vec![json!(id), json!(2), json!(1), value],
    };
    let out = e.event(&report(ids("display:pitch_line"), json!(0)), 1);
    assert!(out.record.is_none(), "a setting, no entry");
    let told = serde_json::to_string(&out.answer).unwrap();
    assert!(told.contains("\"lines\":[]"), "{told}");
    let out = e.event(&report(ids("display:slurs"), json!(4)), 1);
    let told = serde_json::to_string(&out.answer).unwrap();
    assert!(told.contains("\"slur_style\":\"band\""), "{told}");
    // the velocity taken off the tabs leaves no lane drawn
    e.event(&gesture("curve_cc1", vec![]), 1);
    let out = e.event(&report(ids("display:lane:0"), json!(0)), 2);
    let told = serde_json::to_string(&out.answer).unwrap();
    assert!(told.contains("\"CC 1 ch 1\"]"), "{told}");
    assert!(!told.contains("\"lane_tabs\":[\"velocity\""), "{told}");
    let said: Value = serde_json::from_str(&call_json(&mut e, r#"{"verb": "editing"}"#)).unwrap();
    assert_eq!(said["display"]["slurs"], "band");
    assert_eq!(said["display"]["hidden_lanes"], json!(["velocity"]));
    assert_eq!(said["display_panel"], true);
    // a client's setting is checked, and a refused one changes nothing
    let refused = call_json(
        &mut e,
        r#"{"verb": "sync", "display": {"marks": "spiral"}}"#,
    );
    assert!(refused.contains("refused"), "{refused}");
}

/// A roll whose window is its sequence's only holder, every dialog numbered
/// from 100, its window 39 and its roll 40; and its widgets by name.
fn holder(sequence: Shared) -> (NotesEditor, serde_json::Map<String, Value>) {
    let mut e = new_json(sequence, r#"{"rate": 100.0, "version": 1}"#);
    let names: Value = serde_json::from_str(&call_json(&mut e, r#"{"verb": "dialogs"}"#)).unwrap();
    let ids: serde_json::Map<String, Value> = names["dialogs"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, name)| (name.as_str().unwrap().to_string(), json!(100 + i)))
        .collect();
    call_json(
        &mut e,
        &json!({"verb": "window", "widget": 40, "dialogs": ids}).to_string(),
    );
    call_json(&mut e, r#"{"verb": "sync", "window": 39, "asks": true}"#);
    (e, ids)
}

/// **A roll that holds its sequence alone has a file**: Save with none yet
/// asks where, Save with one names it for the holder to write, a close with
/// changes the file does not hold asks first, and Don't save closes.
#[test]
fn a_roll_with_a_file_saves_and_asks_before_a_close_loses_work() {
    let sequence = shared();
    let (mut e, ids) = holder(sequence.clone());
    assert!(e.actions(false, false)["save"].enabled);
    let file = e.window(40)["menu"][0]["menu"].to_string();
    assert!(
        file.contains("open_file") && file.contains("save_as"),
        "{file}"
    );
    // no file yet: the Save as form, offering a file of the format
    let out = e.event(&window_verb("save"), 1);
    assert!(out.save.is_none());
    let told = serde_json::to_string(&out.answer).unwrap();
    assert!(told.contains("untitled.mid"), "{told}");
    let said = |e: &mut NotesEditor, name: &str, tag: &str| {
        let event = Event {
            addr: "/gui_event".into(),
            args: vec![ids[name].clone(), json!(1), json!(0), json!(tag)],
        };
        e.event(&event, 1)
    };
    said(&mut e, "file:path", "/tmp/a roll.mid");
    let out = said(&mut e, "file:ok", "click");
    assert_eq!(out.save.as_deref(), Some("/tmp/a roll.mid"));
    // a path the host said: Save names it at once
    call_json(&mut e, r#"{"verb": "sync", "path": "/tmp/b.mid"}"#);
    let out = e.event(&window_verb("save"), 1);
    assert_eq!(out.save.as_deref(), Some("/tmp/b.mid"));
    // a change the file does not hold, and a close
    assert!(!e.unsaved());
    sequence.lock().unwrap().events.pop();
    assert!(e.unsaved());
    let out = e.event(&window_verb("close"), 2);
    assert!(!out.close, "asked first");
    let out = said(&mut e, "close:discard", "click");
    assert!(out.close, "Don't save closes");
}

/// **A client's roll has no file**: the sequence is its holder's, so its
/// file verbs are refused with why, and its menu has none.
#[test]
fn a_client_s_roll_has_no_file_of_its_own() {
    let mut e = editor(shared());
    let out = e.event(&window_verb("save"), 1);
    let told = serde_json::to_string(&out.answer).unwrap();
    assert!(told.contains("no file of its own"), "{told}");
    assert!(!e.actions(false, false)["open_file"].enabled);
    let tree = e.window(40);
    let file = tree["menu"][0]["menu"].to_string();
    assert!(
        !file.contains("open_file") && !file.contains("\"save\""),
        "{file}"
    );
}

/// **The MIDI mode edits one channel or every one, the window's**: a
/// channel chosen from the menu is the one in force and the roll veils the
/// others' notes; nothing is recorded; a script sets it through `sync`; and
/// under MPE it is refused with why, from the menu and from a script.
#[test]
fn the_midi_mode_edits_the_channel_chosen() {
    let sequence = Arc::new(Mutex::new(EventSequence::new(vec![
        SeqEvent::new(0.0, json!({"midinote": 60, "sustain": 1.0})),
        SeqEvent::new(1.0, json!({"midinote": 64, "sustain": 1.0, "channel": 1})),
    ])));
    let mut e = editor(sequence);
    assert_eq!(e.editing().channel, None, "every channel, at first");
    assert_eq!(e.actions(false, false)["channel_all"].checked, Some(true));
    let out = e.event(&window_verb("channel_2"), 1);
    assert!(out.record.is_none(), "the window's, no edit");
    assert_eq!(e.editing().channel, Some(1));
    assert_eq!(e.actions(false, false)["channel_2"].checked, Some(true));
    let props = call_json(&mut e, r#"{"verb": "props", "widget": 40}"#);
    assert!(props.contains("\"dimmed\":[1,0]"), "{props}");
    call_json(&mut e, r#"{"verb": "sync", "channel": null}"#);
    assert_eq!(e.editing().channel, None);
    // under MPE a channel is one note's
    let mut mpe = EventSequence::new(vec![SeqEvent::new(
        0.0,
        json!({"midinote": 60, "sustain": 1.0, "channel": 1}),
    )]);
    mpe.midi = Some(clausters_document::events::MidiSpec::Mpe {
        upper: false,
        members: 15,
    });
    let mut zone = editor(Arc::new(Mutex::new(mpe)));
    let out = zone.event(&window_verb("channel_2"), 1);
    let told = serde_json::to_string(&out.answer).unwrap();
    assert!(told.contains("under MPE"), "{told}");
    assert_eq!(zone.editing().channel, None);
    assert!(!zone.actions(false, false)["channel_2"].enabled);
    let said = call_json(&mut zone, r#"{"verb": "sync", "channel": 1}"#);
    assert!(said.contains("refused"), "{said}");
}

/// **A curve added while the display panel is up gets its switch**: the
/// panel's lanes no longer being the ones it was composed for, the window is
/// drawn again, and then it has the new lane's switch.
#[test]
fn a_curve_added_with_the_display_panel_up_draws_the_window_again() {
    let sequence = shared();
    let mut e = new_json(sequence.clone(), r#"{"rate": 100.0, "version": 1}"#);
    let names: Value = serde_json::from_str(&call_json(&mut e, r#"{"verb": "dialogs"}"#)).unwrap();
    let numbered: serde_json::Map<String, Value> = names["dialogs"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str().unwrap().to_string(), json!(200 + i as i64)))
        .collect();
    let request = json!({"verb": "window", "widget": 40, "dialogs": numbered}).to_string();
    call_json(&mut e, &request);
    call_json(&mut e, r#"{"verb": "sync", "window": 39}"#);
    assert!(!e.panel_stale(), "hidden, nothing to compose");
    e.event(&window_verb("display_settings"), 1);
    let tree = e.window(40).to_string();
    assert!(!tree.contains("CC 1 ch 1") && !e.panel_stale());
    e.event(&gesture("curve_cc1", vec![]), 1);
    assert!(e.panel_stale(), "a lane with no switch");
    let tree = e.window(40).to_string();
    assert!(tree.contains("\"label\":\"CC 1 ch 1\""), "{tree}");
    assert!(!e.panel_stale());
}
