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
    let p = project(&sequence(), &YDomain::midi(), &Axis::constant(100.0));
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
    let p = project(&seq, &YDomain::midi(), &Axis::constant(100.0));
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
    let p = project(&seq, &domain, &Axis::constant(1.0));
    assert!((p.notes[2] - 261.625_565_300_598_6).abs() < 1e-6);
    let values = report(&[(2, 1.0, 0.5, 880.0, 51.0, 0.0)]);
    let next = applied(
        &seq,
        &intake(&seq, "notes", &values, &Axis::constant(1.0), &domain),
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
    let p = project(&seq, &YDomain::midi(), &axis);
    assert_eq!(&p.notes[0..2], &[0.0, 50.0], "a beat is half a second");
    let values = report(&[(1, 100.0, 50.0, 60.0, 13.0, 0.0)]);
    let next = applied(
        &seq,
        &intake(&seq, "notes", &values, &axis, &YDomain::midi()),
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
        ),
    );
    assert_eq!(next.get(3).unwrap().at.0, 2.5);
    let refused = intake(
        &seq,
        "osc",
        &[json!(250.0), json!("/cue"), json!(10.0), json!("")],
        &Axis::constant(100.0),
        &YDomain::midi(),
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
    let p = project(&s, &YDomain::midi(), &Axis::constant(100.0));
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
/// reader knows it by, and nothing for a sequence for the server.
#[test]
fn a_roll_shows_the_midi_spec() {
    use clausters_document::events::MidiSpec;
    let mut s = sequence();
    assert_eq!(
        project(&s, &YDomain::midi(), &Axis::constant(100.0)).midi,
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
        project(&s, &YDomain::midi(), &Axis::constant(100.0)).midi,
        "MPE"
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
    let p = project(&s, &YDomain::midi(), &Axis::constant(100.0));
    assert_eq!(p.curves[1], json!("CC 74 ch 2"));
}

/// **A `points` report is the one curve it changed**, back in beats.
#[test]
fn a_points_report_is_the_edit_of_the_curve_it_changed() {
    let s = curved();
    let axis = Axis::constant(100.0);
    let p = project(&s, &YDomain::midi(), &axis);
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
    let intake = intake(&s, "points", &values, &axis, &YDomain::midi());
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
    intake(s, "points", values, axis, &YDomain::midi())
}
