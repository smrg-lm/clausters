use serde_json::json;

use super::*;
use clausters_core::tempomap::TempoMap;

fn sequence() -> EventSequence {
    EventSequence::new(vec![
        Event::new(
            0.0,
            json!({"midinote": 60, "sustain": 1.0, "amp": 0.1, "instrument": "bell"}),
        ),
        Event::new(
            1.0,
            json!({"freq": 440.0, "midinote": 69, "sustain": 0.5, "amp": 0.4}),
        ),
        Event::new(2.0, json!({"type": "osc", "addr": "/cue", "args": [1]})),
        Event::new(3.0, json!({"degree": 2, "sustain": 1.0})),
    ])
}

fn report(notes: &[(u64, f64, f64, f64, f64, f64)]) -> Vec<Value> {
    notes
        .iter()
        .flat_map(|&(id, a, b, c, d, e)| {
            [json!(id), json!(a), json!(b), json!(c), json!(d), json!(e)]
        })
        .collect()
}

fn applied(sequence: &EventSequence, intake: &Intake) -> EventSequence {
    let mut next = sequence.clone();
    let intent: EventsIntent = serde_json::from_value(intake.payloads[0].clone()).unwrap();
    next.edit(intent).unwrap();
    next
}

#[test]
fn a_roll_draws_each_note_with_its_id_where_it_sounds() {
    let p = project(
        &sequence(),
        &YDomain::midi(),
        &Axis::constant(100.0),
        &Editing::default(),
    );
    assert_eq!(p.note_ids, vec![1, 2, 4]);
    assert_eq!(&p.notes[0..5], &[0.0, 100.0, 60.0, 13.0, 0.0]);
    assert!(
        (p.notes[7] - 69.0).abs() < 1e-9,
        "the freq a note was written with"
    );
    assert_eq!(p.notes[12], 64.0, "and the degree");
    assert_eq!(p.osc, vec![json!(200.0), json!("/cue")]);
}

#[test]
fn an_untouched_report_changes_nothing() {
    let seq = sequence();
    let p = project(
        &seq,
        &YDomain::midi(),
        &Axis::constant(100.0),
        &Editing::default(),
    );
    let values: Vec<Value> = p
        .note_ids
        .iter()
        .zip(p.notes.chunks(5))
        .flat_map(|(id, n)| std::iter::once(json!(id)).chain(n.iter().map(|v| json!(v))))
        .collect();
    let intake = intake(
        &seq,
        "notes",
        &values,
        &Axis::constant(100.0),
        &YDomain::midi(),
        &Editing::default(),
    );
    let mut next = seq.clone();
    let intent: EventsIntent = serde_json::from_value(intake.payloads[0].clone()).unwrap();
    assert!(
        !next.edit(intent).unwrap().applied,
        "every key kept, amplitudes included"
    );
}

/// Order is no identity: the note removed is the one named, and every other
/// keeps its own keys -- its instrument, its amplitude.
#[test]
fn removing_a_note_leaves_its_neighbours_theirs() {
    let seq = sequence();
    let values = report(&[
        (2, 100.0, 50.0, 69.0, 51.0, 0.0),
        (4, 300.0, 100.0, 64.0, 13.0, 0.0),
    ]);
    let next = applied(
        &seq,
        &intake(
            &seq,
            "notes",
            &values,
            &Axis::constant(100.0),
            &YDomain::midi(),
            &Editing::default(),
        ),
    );
    assert!(next.get(1).is_none());
    assert_eq!(next.get(2).unwrap().data.0["amp"], json!(0.4));
    assert!(next.get(3).is_some(), "the OSC markers are carried through");
}

#[test]
fn a_moved_note_writes_its_key_with_coherence() {
    let seq = sequence();
    let values = report(&[
        (1, 0.0, 100.0, 60.0, 13.0, 0.0),
        (2, 150.0, 50.0, 72.0, 51.0, 0.0),
        (4, 300.0, 100.0, 66.0, 13.0, 0.0),
    ]);
    let next = applied(
        &seq,
        &intake(
            &seq,
            "notes",
            &values,
            &Axis::constant(100.0),
            &YDomain::midi(),
            &Editing::default(),
        ),
    );
    let moved = next.get(2).unwrap();
    assert_eq!(moved.at.0, 1.5);
    assert!((moved.data.0["freq"].as_f64().unwrap() - 523.251_130_601_197_3).abs() < 1e-9);
    let by_degree = next.get(4).unwrap();
    assert_eq!(
        (
            by_degree.data.0["degree"].as_f64(),
            by_degree.data.0["alter"].as_f64()
        ),
        (Some(3.0), Some(1.0))
    );
    assert_eq!(next.get(1).unwrap().data.0["instrument"], json!("bell"));
}

#[test]
fn a_new_note_and_a_new_velocity() {
    let seq = sequence();
    let values = report(&[
        (1, 0.0, 100.0, 60.0, 100.0, 0.0),
        (0, 400.0, 50.0, 67.0, 90.0, 2.0),
    ]);
    let next = applied(
        &seq,
        &intake(
            &seq,
            "notes",
            &values,
            &Axis::constant(100.0),
            &YDomain::midi(),
            &Editing::default(),
        ),
    );
    let first = next.get(1).unwrap();
    assert_eq!(first.data.0["velocity"], json!(100.0));
    assert!((first.data.0["amp"].as_f64().unwrap() - 100.0 / 127.0).abs() < 1e-12);
    let made = next.events.iter().find(|e| e.at.0 == 4.0).unwrap();
    assert_eq!(made.id, 5);
    assert_eq!(made.data.0["channel"], json!(2.0));
    assert!(
        next.get(2).is_none() && next.get(4).is_none(),
        "the others were left out"
    );
}

#[test]
fn a_roll_in_hz_reads_and_writes_the_frequency() {
    let seq = sequence();
    let domain = YDomain::hz(20.0, 20000.0);
    let p = project(&seq, &domain, &Axis::constant(1.0), &Editing::default());
    assert!((p.notes[2] - 261.625_565_300_598_6).abs() < 1e-6);
    let values = report(&[(2, 1.0, 0.5, 880.0, 51.0, 0.0)]);
    let next = applied(
        &seq,
        &intake(
            &seq,
            "notes",
            &values,
            &Axis::constant(1.0),
            &domain,
            &Editing::default(),
        ),
    );
    assert!((next.get(2).unwrap().data.0["midinote"].as_f64().unwrap() - 81.0).abs() < 1e-9);
}

/// A tempo that changes along the sequence moves where each beat is drawn, and
/// a drag is read back through the same map.
#[test]
fn the_axis_is_the_tempo_map() {
    let mut seq = sequence();
    seq.tempo_map = Some(TempoMap::new(2.0));
    let axis = Axis::of(&seq, 100.0);
    let p = project(&seq, &YDomain::midi(), &axis, &Editing::default());
    assert_eq!(&p.notes[0..2], &[0.0, 50.0], "a beat is half a second");
    let values = report(&[(1, 100.0, 50.0, 60.0, 13.0, 0.0)]);
    let next = applied(
        &seq,
        &intake(
            &seq,
            "notes",
            &values,
            &axis,
            &YDomain::midi(),
            &Editing::default(),
        ),
    );
    assert_eq!(next.get(1).unwrap().at.0, 2.0);
}

#[test]
fn a_marker_moves_by_its_label_and_a_new_one_is_refused() {
    let seq = sequence();
    let next = applied(
        &seq,
        &intake(
            &seq,
            "osc",
            &[json!(250.0), json!("/cue")],
            &Axis::constant(100.0),
            &YDomain::midi(),
            &Editing::default(),
        ),
    );
    assert_eq!(next.get(3).unwrap().at.0, 2.5);
    let refused = intake(
        &seq,
        "osc",
        &[json!(250.0), json!("/cue"), json!(10.0), json!("")],
        &Axis::constant(100.0),
        &YDomain::midi(),
        &Editing::default(),
    );
    assert!(refused.refusal.is_some());
}

/// A sequence with a CC curve and a bend on its first note.
fn curved() -> EventSequence {
    use clausters_document::{NodeId, Point};
    let point = |at: f64, value: f64| Point {
        at: Beat(at),
        value,
        data: Opaque::none(),
    };
    let mut s = sequence();
    let mut cc = Automation::new(NodeId(0), Opaque(json!({"cc": 74})));
    cc.points = vec![point(0.0, 0.0), point(2.0, 127.0)];
    s.edit(EventsIntent::Automation { automation: cc }).unwrap();
    let mut bend = Automation::new(NodeId(0), Opaque(json!({"bend": true})));
    bend.points = vec![point(0.0, 0.0), point(0.5, 1.0)];
    s.edit(EventsIntent::EventAutomation {
        id: 1,
        automation: bend,
    })
    .unwrap();
    s
}

/// **A sequence curve is a row and a note's curve a layer over it**, each point in view
/// units -- a note's measured from the note's start.
#[test]
fn a_roll_draws_the_sequence_curves_and_each_notes_curves() {
    let s = curved();
    let p = project(
        &s,
        &YDomain::midi(),
        &Axis::constant(100.0),
        &Editing::default(),
    );
    let cc = s.automation[0].id.0.to_string();
    let bend = s.events[0].automation[0].id.0.to_string();
    assert_eq!(
        p.curves,
        vec![
            json!(cc),
            json!("CC 74"),
            json!(0.0),
            json!(127.0),
            json!(CURVE_H)
        ]
    );
    assert_eq!(
        p.layers,
        vec![
            json!(bend),
            json!(1),
            json!("bend"),
            json!(-2.0),
            json!(2.0),
            json!(true)
        ]
    );
    // The CC's second point at beat 2 is sample 200; the bend's at half a
    // beat from its note is 50.
    let at_of = |name: &str| -> Vec<f64> {
        p.points
            .chunks(5)
            .filter(|c| c[0] == json!(name))
            .map(|c| c[1].as_f64().unwrap())
            .collect()
    };
    assert_eq!(at_of(&cc), vec![0.0, 200.0]);
    assert_eq!(at_of(&bend), vec![0.0, 50.0]);
}

/// **A roll shows the MIDI spec its sequence is written for**, by the name a
/// reader knows it by, and nothing for a sequence for the server or in the
/// notation mode.
#[test]
fn a_roll_shows_the_midi_spec() {
    use clausters_document::events::MidiSpec;
    let mut s = sequence();
    assert_eq!(
        project(
            &s,
            &YDomain::midi(),
            &Axis::constant(100.0),
            &Editing::default()
        )
        .midi,
        ""
    );
    s.edit(EventsIntent::Midi {
        midi: Some(MidiSpec::Mpe {
            upper: false,
            members: 15,
        }),
    })
    .unwrap();
    assert_eq!(
        project(
            &s,
            &YDomain::midi(),
            &Axis::constant(100.0),
            &Editing::default()
        )
        .midi,
        "MPE"
    );
    // a roll in notation mode shows what a page writes, and no spec
    assert_eq!(
        project(
            &s,
            &YDomain::midi(),
            &Axis::constant(100.0),
            &notation_editing()
        )
        .midi,
        ""
    );
}

/// **A sequence curve names its channel**, counted from 1 as MIDI shows one.
#[test]
fn a_sequence_curve_on_a_channel_says_which() {
    let mut s = sequence();
    let cc = Automation::new(
        clausters_document::NodeId(0),
        Opaque(json!({"cc": 74, "channel": 1})),
    );
    s.edit(EventsIntent::Automation { automation: cc }).unwrap();
    let p = project(
        &s,
        &YDomain::midi(),
        &Axis::constant(100.0),
        &Editing::default(),
    );
    assert_eq!(p.curves[1], json!("CC 74 ch 2"));
}

/// **A `points` report is the one curve it changed**, back in beats.
#[test]
fn a_points_report_is_the_edit_of_the_curve_it_changed() {
    let s = curved();
    let axis = Axis::constant(100.0);
    let p = project(&s, &YDomain::midi(), &axis, &Editing::default());
    let bend = s.events[0].automation[0].id;
    // The report as drawn, with the bend's last point moved to 3 semitones at
    // three quarters of a beat.
    let mut values = p.points.clone();
    let last = values
        .chunks(5)
        .rposition(|c| c[0] == json!(bend.0.to_string()))
        .unwrap();
    values[last * 5 + 1] = json!(75.0);
    values[last * 5 + 2] = json!(3.0);
    let intake = intake(
        &s,
        "points",
        &values,
        &axis,
        &YDomain::midi(),
        &Editing::default(),
    );
    let intent: EventsIntent = serde_json::from_value(intake.payloads[0].clone()).unwrap();
    let EventsIntent::EventAutomation { id, automation } = intent else {
        panic!("a note's curve");
    };
    assert_eq!((id, automation.id), (1, bend));
    assert_eq!(automation.points[1].at, Beat(0.75));
    assert_eq!(automation.points[1].value, 3.0);
    // An unchanged report is no edit.
    let same = intake_of(&s, &p.points, &axis);
    assert!(same.payloads.is_empty());
}

fn intake_of(s: &EventSequence, values: &[Value], axis: &Axis) -> Intake {
    intake(
        s,
        "points",
        values,
        axis,
        &YDomain::midi(),
        &Editing::default(),
    )
}

/// **The bars are the page's**: a sequence with no meter has none, one with a
/// reading's meter has its bars, and a grid's meters by measure, an
/// anacrusis and an irregular bar each start a run of their own.
#[test]
fn the_bars_are_the_pages_meters_by_measure() {
    let mut s = sequence();
    assert!(bars(&s).is_empty(), "no meter, no bars of the sequence's");
    s.notation = Some(json!({"reading": {"meter": "3/4"}}));
    assert_eq!(bars(&s), vec![0.0, 3.0, 1.0]);
    s.notation = Some(json!({"grid": {
        "meters": [{"measure": 0, "count": 4, "unit": 4}, {"measure": 3, "count": 6, "unit": 8}],
        "irregular": [[0, [1, 4]], [5, [1, 8]]],
    }}));
    assert_eq!(
        bars(&s),
        vec![
            0.0, 1.0, 0.0, // the pickup, bar 0
            1.0, 4.0, 1.0, // 4/4 from bar 1
            9.0, 3.0, 3.0, // 6/8 from measure 3
            15.0, 0.5, 5.0, // an eighth's bar
            15.5, 3.0, 6.0, // and 6/8 again
        ]
    );
    let p = project(
        &s,
        &YDomain::midi(),
        &Axis::constant(1.0),
        &Editing::default(),
    );
    assert_eq!(p.bars, bars(&s));
}

/// Three notes of one line, a beat each, and one on another channel.
fn line() -> EventSequence {
    EventSequence::new(vec![
        Event::new(0.0, json!({"midinote": 60, "sustain": 1.0, "dur": 1.0})),
        Event::new(
            1.0,
            json!({"midinote": 62, "sustain": 1.0, "dur": 1.0, "value": [1, 4]}),
        ),
        Event::new(2.0, json!({"midinote": 64, "sustain": 1.0, "dur": 1.0})),
        Event::new(1.0, json!({"midinote": 40, "sustain": 2.0, "channel": 1})),
    ])
}

/// The sequence a `notes` report leaves, with the editing options given.
fn reported(s: &EventSequence, values: &[Value], editing: &Editing) -> EventSequence {
    let intake = intake(
        s,
        "notes",
        values,
        &Axis::constant(1.0),
        &YDomain::midi(),
        editing,
    );
    let mut after = s.clone();
    after
        .edit(serde_json::from_value(intake.payloads[0].clone()).unwrap())
        .unwrap();
    after
}

/// **Editing monophonically, the gesture wins over the notes it covers**:
/// lengthened over the next, the first shortens it from its start; over the
/// next whole, it takes it away; and another line is not touched. Edited
/// polyphonically, nothing is shortened.
#[test]
fn a_monophonic_gesture_shortens_what_it_covers_in_its_line() {
    let s = line();
    let p = project(
        &s,
        &YDomain::midi(),
        &Axis::constant(1.0),
        &Editing::default(),
    );
    let mono = Editing {
        monophonic: true,
        ..Editing::default()
    };
    let by = |m: i64| {
        s.events
            .iter()
            .find(|e| e.keys()["midinote"] == json!(m))
            .unwrap()
            .id
    };
    let first = p.note_ids.iter().position(|id| *id == by(60)).unwrap();
    // the first note lengthened to a beat and a half
    let mut values = Vec::new();
    for (i, id) in p.note_ids.iter().enumerate() {
        let q = &p.notes[i * 5..i * 5 + 5];
        let dur = if i == first { 1.5 } else { q[1] };
        values.extend([
            json!(id),
            json!(q[0]),
            json!(dur),
            json!(q[2]),
            json!(q[3]),
            json!(q[4]),
        ]);
    }
    let after = reported(&s, &values, &mono);
    let second = after.get(by(62)).unwrap();
    assert_eq!(second.at, Beat(1.5), "its onset moves to the new end");
    assert_eq!(render::sustain_of(&second.keys()), 0.5, "and its end stays");
    assert!(
        second.keys().get("value").is_none(),
        "its written value goes"
    );
    let bass = after.get(by(40)).unwrap();
    assert_eq!(
        (bass.at, render::sustain_of(&bass.keys())),
        (Beat(1.0), 2.0)
    );
    // polyphonically, the two overlap
    let poly = reported(&s, &values, &Editing::default());
    assert_eq!(poly.get(by(62)).unwrap().at, Beat(1.0));
    // lengthened over the second whole, it takes it away
    values[first * 6 + 2] = json!(2.0);
    let covered = reported(&s, &values, &mono);
    assert!(covered.get(by(62)).is_none());
    assert_eq!(covered.events.len(), 3);
}

/// **A join is drawn as a link**, and `join` and `unjoin` are the sequence's
/// own two edits over the notes named.
#[test]
fn joins_are_drawn_and_edited_by_the_notes_named() {
    let s = line();
    let ids: Vec<u64> = s.events.iter().map(|e| e.id).collect();
    let axis = Axis::constant(1.0);
    let join = intake(
        &s,
        "join",
        &[json!(ids[0]), json!(ids[1])],
        &axis,
        &YDomain::midi(),
        &Editing::default(),
    );
    let mut joined = s.clone();
    joined
        .edit(serde_json::from_value(join.payloads[0].clone()).unwrap())
        .unwrap();
    assert_eq!(join.label, "join");
    let p = project(&joined, &YDomain::midi(), &axis, &Editing::default());
    let at = |id: u64| p.note_ids.iter().position(|n| *n == id).unwrap();
    assert_eq!(p.joins, vec![at(ids[0]), at(ids[1])]);
    let unjoin = intake(
        &joined,
        "unjoin",
        &[json!(ids[1])],
        &axis,
        &YDomain::midi(),
        &Editing::default(),
    );
    joined
        .edit(serde_json::from_value(unjoin.payloads[0].clone()).unwrap())
        .unwrap();
    assert!(
        project(&joined, &YDomain::midi(), &axis, &Editing::default())
            .joins
            .is_empty()
    );
    let none = intake(
        &s,
        "join",
        &[],
        &axis,
        &YDomain::midi(),
        &Editing::default(),
    );
    assert_eq!(none.refusal.as_deref(), Some("nothing is selected"));
}

/// A line written as notation: three quarters on staff 0, voice 0.
#[cfg(feature = "notation")]
fn written_line() -> EventSequence {
    let mut s = EventSequence::new(vec![
        Event::new(
            0.0,
            json!({"midinote": 60, "dur": 1.0, "value": [1, 4], "staff": 0, "voice": 0}),
        ),
        Event::new(
            1.0,
            json!({"midinote": 60, "dur": 1.0, "value": [1, 4], "staff": 0, "voice": 0}),
        ),
        Event::new(
            2.0,
            json!({"midinote": 64, "dur": 1.0, "value": [1, 4], "staff": 0, "voice": 0}),
        ),
    ]);
    s.notation = Some(json!({"grid": {"meters": [{"measure": 0, "count": 4, "unit": 4}]}}));
    s
}

fn notation_editing() -> Editing {
    Editing {
        mode: Mode::Notation,
        ..Editing::default()
    }
}

/// The sequence a gesture in notation mode leaves, or why it was refused.
#[cfg(feature = "notation")]
fn in_notation(
    s: &EventSequence,
    tag: &str,
    values: &[Value],
    editing: &Editing,
) -> Result<EventSequence, String> {
    let intake = intake(
        s,
        tag,
        values,
        &Axis::constant(1.0),
        &YDomain::midi(),
        editing,
    );
    if let Some(why) = intake.refusal {
        return Err(why);
    }
    let mut after = s.clone();
    after
        .edit(serde_json::from_value(intake.payloads[0].clone()).unwrap())
        .unwrap();
    Ok(after)
}

/// The report of every note of `s` as the roll in `editing` draws it, with
/// `more` sextuples after.
#[cfg(feature = "notation")]
fn drawn_report(s: &EventSequence, editing: &Editing, more: &[f64]) -> Vec<Value> {
    let p = project(s, &YDomain::midi(), &Axis::constant(1.0), editing);
    let mut values = Vec::new();
    for (i, id) in p.note_ids.iter().enumerate() {
        values.push(json!(id));
        values.extend(p.notes[i * 5..i * 5 + 5].iter().map(|v| json!(v)));
    }
    values.extend(more.iter().map(|v| json!(v)));
    values
}

/// **A note in notation mode is written**: it lands on the reading's grid
/// and a written value, in the voice edited, its pitch spelled; what it
/// sounds is derived; and a velocity moved is written as a dynamic.
#[cfg(feature = "notation")]
#[test]
fn a_note_in_notation_mode_lands_on_the_grid_and_a_written_value() {
    let s = written_line();
    let editing = Editing {
        voice: [0, 1],
        ..notation_editing()
    };
    let p = project(&s, &YDomain::midi(), &Axis::constant(1.0), &editing);
    assert_eq!(p.voices, vec![0, 0, 0, 0, 0, 0]);
    assert_eq!(p.dimmed, vec![1, 1, 1], "another voice is edited");
    let after = in_notation(
        &s,
        "notes",
        &drawn_report(&s, &editing, &[0.0, 3.05, 0.7, 67.0, 64.0, 0.0]),
        &editing,
    )
    .unwrap();
    let new = after
        .events
        .iter()
        .find(|e| e.keys()["midinote"] == json!(67.0))
        .unwrap();
    let keys = new.keys();
    assert_eq!(new.at, Beat(3.0));
    assert_eq!(keys["value"], json!([3, 16]), "a dotted eighth");
    assert_eq!(
        (keys["staff"].clone(), keys["voice"].clone()),
        (json!(0), json!(1))
    );
    assert_eq!(keys["pitches"][0]["step"], json!("g"));
    assert!(keys.get("sustain").is_some(), "derived");
    // the velocity of the first, moved: a dynamic
    let mut values = drawn_report(&after, &editing, &[]);
    values[4] = json!(120.0);
    let louder = in_notation(&after, "notes", &values, &editing).unwrap();
    let dynamic = louder.events[0].keys()["dynamic"].clone();
    assert!(
        dynamic.as_str().is_some_and(|d| d.starts_with('f')),
        "{dynamic}"
    );
}

/// **Two notes of one voice overlap only as a chord** where the part writes
/// voices, and freely where it writes ties.
#[cfg(feature = "notation")]
#[test]
fn notes_of_one_voice_overlap_only_as_a_chord_unless_written_with_ties() {
    let s = written_line();
    let editing = notation_editing();
    // the first lengthened over the second
    let mut values = drawn_report(&s, &editing, &[]);
    values[2] = json!(1.5);
    let why = in_notation(&s, "notes", &values, &editing).unwrap_err();
    assert!(why.contains("overlap"), "{why}");
    // a chord: a note that starts and ends with the first
    let chord = drawn_report(&s, &editing, &[0.0, 0.0, 1.0, 64.0, 64.0, 0.0]);
    assert!(in_notation(&s, "notes", &chord, &editing).is_ok());
    // written with ties, the overlap is kept
    let mut ties = s.clone();
    ties.parts = vec![
        serde_json::from_value(json!({"writing": {"polyphony": "ties", "voices_per_staff": 1}}))
            .unwrap(),
    ];
    assert!(in_notation(&ties, "notes", &values, &editing).is_ok());
    // monophonically, the overlap shortens the second instead
    let mono = Editing {
        monophonic: true,
        ..notation_editing()
    };
    let after = in_notation(&s, "notes", &values, &mono).unwrap();
    let second = &after.events[1];
    assert_eq!(
        (second.at, second.keys()["value"].clone()),
        (Beat(1.5), json!([1, 8]))
    );
}

/// **The page's marks over a selection**: a staccato toggles, a tie makes
/// two notes of one pitch that touch one note, a slur is drawn joined where
/// the roll edits monophonically, and each is heard at once.
#[cfg(feature = "notation")]
#[test]
fn the_pages_marks_are_written_over_the_selection() {
    let s = written_line();
    let ids: Vec<Value> = s.events.iter().map(|e| json!(e.id)).collect();
    let editing = notation_editing();
    let short = in_notation(&s, "staccato", &ids[..1], &editing).unwrap();
    assert_eq!(short.events[0].keys()["articulations"], json!(["stacc"]));
    assert!(
        render::sustain_of(&short.events[0].keys()) < render::sustain_of(&short.events[1].keys()),
        "and heard short"
    );
    let back = in_notation(&short, "staccato", &ids[..1], &editing).unwrap();
    assert!(back.events[0].keys().get("articulations").is_none());
    // a tie: the two Cs are one note, a half
    let tied = in_notation(&s, "tie", &ids[..2], &editing).unwrap();
    assert_eq!(tied.events.len(), 2);
    assert_eq!(tied.events[0].keys()["value"], json!([1, 2]));
    assert!(
        in_notation(&s, "tie", &ids[1..], &editing).is_err(),
        "another pitch"
    );
    // a slur, joined where the roll edits monophonically
    let mono = Editing {
        monophonic: true,
        ..notation_editing()
    };
    let slurred = in_notation(&s, "slur", &ids[1..], &mono).unwrap();
    let spanner = slurred.notation.as_ref().unwrap()["spanners"][0].clone();
    assert_eq!(spanner["join"], json!(true));
    assert_eq!(slurred.events[2].keys()["joins"], ids[1]);
    let attacked = in_notation(&slurred, "slur_join", &ids[1..], &mono).unwrap();
    assert!(attacked.events[2].keys().get("joins").is_none());
    // in MIDI mode a mark is refused
    let refused = intake(
        &s,
        "slur",
        &ids,
        &Axis::constant(1.0),
        &YDomain::midi(),
        &Editing::default(),
    );
    assert!(refused.refusal.is_some());
}

/// The written value of a length in beats.
#[test]
fn a_length_is_written_as_its_value() {
    assert_eq!(notation::value_of(1.5, 4), Some(json!([3, 8])));
    assert_eq!(notation::value_of(1.0 / 3.0, 4), Some(json!([1, 12])));
    assert_eq!(notation::value_of(0.0, 4), None);
}

/// **A piano part edited in notation mode is the page the score editor
/// reads**: a note drawn in the lower staff's voice is on that staff of the
/// page, where it is written, with nothing read anew.
#[cfg(feature = "notation")]
#[test]
fn a_piano_part_edited_in_notation_mode_is_its_page() {
    use clausters_core::notation::{Item, default_interpretation};
    use clausters_document::events::reading::Transcription;
    use clausters_document::events::transcription::read;

    let mut take = EventSequence::new(vec![
        Event::new(0.0, json!({"midinote": 72, "dur": 2.0})),
        Event::new(0.0, json!({"midinote": 48, "dur": 2.0})),
    ]);
    take.parts = vec![serde_json::from_value(json!({"name": "Piano", "staves": 2})).unwrap()];
    take.edit(EventsIntent::ToNotation {
        reading: Some(json!({"meter": "4/4"})),
        writing: None,
        interpretation: None,
    })
    .unwrap();
    let editing = Editing {
        voice: [1, 0],
        ..notation_editing()
    };
    let after = in_notation(
        &take,
        "notes",
        &drawn_report(&take, &editing, &[0.0, 2.0, 2.0, 43.0, 64.0, 0.0]),
        &editing,
    )
    .unwrap();
    let page = read(
        &after,
        &Transcription::of(&after),
        &default_interpretation(),
    )
    .unwrap()
    .sheet;
    let lower: Vec<(Vec<i32>, f64)> = page.staves[1].voices[0]
        .items
        .iter()
        .map(|item| {
            (
                item.pitches().iter().map(|p| p.midi()).collect(),
                item.dur().to_f64(),
            )
        })
        .collect();
    assert_eq!(lower, vec![(vec![48], 0.5), (vec![43], 0.5)]);
    assert!(matches!(
        page.staves[0].voices[0].items[0],
        Item::Note { .. }
    ));
}

/// The sequence a roll's report leaves, or why it was refused -- in any mode.
fn answered(
    s: &EventSequence,
    tag: &str,
    values: &[Value],
    editing: &Editing,
) -> Result<EventSequence, String> {
    let intake = intake(
        s,
        tag,
        values,
        &Axis::constant(1.0),
        &YDomain::midi(),
        editing,
    );
    if let Some(why) = intake.refusal {
        return Err(why);
    }
    let mut after = s.clone();
    after
        .edit(serde_json::from_value(intake.payloads[0].clone()).unwrap())
        .unwrap();
    Ok(after)
}

/// **A transposition in MIDI mode is by semitones and octaves**, of the
/// selection or of every note, what it sounds moved with its family.
#[test]
fn a_transposition_in_midi_mode_moves_the_selection_or_every_note() {
    let s = sequence();
    let midi = |s: &EventSequence, i: usize| {
        let keys = s.events[i].keys();
        render::pitch_of(&keys).midinote(&render::scale_of(&keys))
    };
    let first = s.events[0].id;
    let after = answered(&s, "step_up", &[json!(first)], &Editing::default()).unwrap();
    assert_eq!(midi(&after, 0), 61.0);
    assert_eq!(midi(&after, 1), 69.0, "a note not selected stays");
    assert_eq!(after.events[0].keys()["instrument"], json!("bell"));
    let after = answered(&s, "octave_down", &[], &Editing::default()).unwrap();
    assert_eq!(midi(&after, 0), 48.0);
    assert_eq!(midi(&after, 1), 57.0);
    assert!((after.events[1].keys()["freq"].as_f64().unwrap() - 220.0).abs() < 1e-6);
    assert_eq!(after.events[2], s.events[2], "a marker is no note");
    let empty = EventSequence::new(vec![]);
    assert!(answered(&empty, "step_up", &[], &Editing::default()).is_err());
}

/// **A step in notation mode is a step of the key**: the next letter, with
/// the sign the key writes on it, and what it sounds follows.
#[test]
fn a_step_in_notation_mode_is_a_step_of_the_key() {
    let mut s = EventSequence::new(vec![Event::new(
        0.0,
        json!({"midinote": 64, "dur": 1.0, "value": [1, 4], "staff": 0, "voice": 0,
               "pitches": [{"step": "e", "alter": 0, "octave": 4}]}),
    )]);
    s.notation = Some(json!({"key": "D"}));
    let editing = notation_editing();
    let after = answered(&s, "step_up", &[], &editing).unwrap();
    let keys = after.events[0].keys();
    assert_eq!(
        keys["pitches"],
        json!([{"step": "f", "alter": 1, "octave": 4}])
    );
    assert_eq!(keys["midinote"].as_f64(), Some(66.0));
    let after = answered(&s, "octave_up", &[], &editing).unwrap();
    let keys = after.events[0].keys();
    assert_eq!(keys["pitches"][0]["step"], json!("e"));
    assert_eq!(keys["midinote"].as_f64(), Some(76.0));
}

/// **A selection by data marks every note sharing it**: a pitch, a
/// channel, a voice, among the notes the roll takes.
#[test]
fn a_selection_by_data_marks_every_note_that_shares_it() {
    let s = EventSequence::new(vec![
        Event::new(
            0.0,
            json!({"midinote": 60, "channel": 1, "staff": 0, "voice": 0}),
        ),
        Event::new(
            1.0,
            json!({"midinote": 64, "channel": 2, "staff": 0, "voice": 1}),
        ),
        Event::new(
            2.0,
            json!({"midinote": 60, "channel": 2, "staff": 1, "voice": 0}),
        ),
        Event::new(3.0, json!({"type": "osc", "addr": "/cue"})),
    ]);
    let id = |i: usize| s.events[i].id;
    let like =
        |tag: &str, ids: &[u64]| select_like(&s, tag, ids, &YDomain::midi(), &Editing::default());
    assert_eq!(like("select_pitch", &[id(0)]), vec![id(0), id(2)]);
    assert_eq!(like("select_channel", &[id(1)]), vec![id(1), id(2)]);
    assert_eq!(like("select_voice", &[id(0)]), vec![id(0)]);
    assert_eq!(like("select_staff", &[id(1)]), vec![id(0), id(1)]);
    assert!(
        like("select_pitch", &[id(3)]).is_empty(),
        "a marker is no note"
    );
    // in notation mode, only the voice edited is taken
    let notation = notation_editing();
    assert_eq!(
        select_like(&s, "select_pitch", &[id(0)], &YDomain::midi(), &notation),
        vec![id(0)]
    );
}

/// **A split is the document's in either mode**: the cut in the view's
/// units and the notes, one intent; in notation mode the halves' written
/// values are read again.
#[test]
fn a_split_is_the_documents_in_either_mode() {
    let s = EventSequence::new(vec![Event::new(
        0.0,
        json!({"midinote": 60, "dur": 2.0, "value": [1, 2], "staff": 0, "voice": 0}),
    )]);
    let id = s.events[0].id;
    for editing in [Editing::default(), notation_editing()] {
        let after = answered(&s, "split", &[json!(1.0), json!(id)], &editing).unwrap();
        assert_eq!(after.events.len(), 2);
        assert!(
            after
                .events
                .iter()
                .all(|e| e.keys()["value"] == json!([1, 4]))
        );
    }
    assert!(answered(&s, "split", &[json!(1.0)], &Editing::default()).is_err());
}
