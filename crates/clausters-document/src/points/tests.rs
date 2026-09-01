use super::*;
use crate::history::History;

fn at(at: f64, value: f64) -> Point {
    Point { at, value }
}

fn set(points: Vec<Point>) -> Opaque {
    payload(&PointsIntent::SetPoints { points })
}

#[test]
fn a_curve_edited_through_a_history_inverts_to_the_points_it_started_from() {
    let mut history = History::new();
    let curve = history.register(POINTS);
    let mut points = Points::new(vec![at(0.0, 0.0), at(1.0, 1.0)]);

    let applied = history.apply(
        curve,
        &mut points,
        &set(vec![at(0.0, 0.0), at(1.0, 0.5), at(2.0, 1.0)]),
        "draw",
    );
    assert!(applied.applied);
    assert_eq!(points.0.len(), 3);

    for (structure, payload) in history.undo().expect("something to undo").legs {
        assert_eq!(structure, curve);
        points.apply(&payload);
    }
    assert_eq!(
        points.0,
        vec![at(0.0, 0.0), at(1.0, 1.0)],
        "where it started"
    );
}

#[test]
fn a_resend_is_not_an_edit_and_leaves_no_entry() {
    let mut history = History::new();
    let curve = history.register(POINTS);
    let mut points = Points::new(vec![at(0.0, 0.0)]);

    let applied = history.apply(curve, &mut points, &set(vec![at(0.0, 0.0)]), "draw");
    assert!(!applied.applied);
    assert!(history.is_empty(), "a resend does not become an undo step");
}

#[test]
fn an_edit_in_another_domains_vocabulary_is_refused_with_what_holds() {
    let mut history = History::new();
    let curve = history.register(POINTS);
    let mut points = Points::new(vec![at(0.0, 0.25)]);

    // The arrangement's own verb, sent to a curve. It names a node, which a
    // curve has none of, so the refusal is the whole of what routing by domain
    // exists to prevent -- and it says so rather than doing nothing.
    let intent = Opaque(serde_json::json!({"intent": "place", "node": 1, "offset": 0.0}));
    let applied = history.apply(curve, &mut points, &intent, "place");
    assert!(!applied.applied);
    assert!(applied.reason.is_some());
    assert_eq!(applied.effective, set(vec![at(0.0, 0.25)]));
    assert!(history.is_empty());
}

#[test]
fn a_structure_this_history_did_not_mint_is_refused_at_apply_too() {
    // The rule holds on the applying door as well as the recording one:
    // otherwise a composed view could edit through a history that does not hold
    // its data, and the entry would be the only thing to say so.
    let mut history = History::new();
    let elsewhere = History::new().register(POINTS);
    let mut points = Points::new(vec![at(0.0, 0.0)]);

    let applied = history.apply(elsewhere, &mut points, &set(vec![at(1.0, 1.0)]), "draw");
    assert!(!applied.applied);
    assert_eq!(points.0, vec![at(0.0, 0.0)], "and it did not edit either");
}

#[test]
fn a_redo_hands_the_curve_back_the_points_it_had() {
    let mut history = History::new();
    let curve = history.register(POINTS);
    let mut points = Points::new(vec![at(0.0, 0.0)]);
    history.apply(curve, &mut points, &set(vec![at(0.0, 1.0)]), "draw");

    for (_, payload) in history.undo().unwrap().legs {
        points.apply(&payload);
    }
    assert_eq!(points.0, vec![at(0.0, 0.0)]);
    for (_, payload) in history.redo().unwrap().edits {
        points.apply(&payload);
    }
    assert_eq!(points.0, vec![at(0.0, 1.0)]);
}
