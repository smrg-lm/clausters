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
    assert!(next.get(3).is_some(), "the marker lane is carried through");
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
