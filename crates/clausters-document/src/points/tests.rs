use super::*;
use crate::history::History;

fn at(at: f64, value: f64) -> Point {
    Point {
        at,
        value,
        data: Opaque::default(),
    }
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

#[test]
fn a_point_carries_what_the_client_says_about_it_and_the_crate_reads_none_of_it() {
    // The segment between two points has a shape, and it belongs to the point
    // that starts it. Deciding what a shape *is* stays refused; carrying one is
    // not the same act, and dropping it made an undo straighten the curve it
    // was putting back.
    let shaped = Point {
        at: 0.0,
        value: 1.0,
        data: Opaque(serde_json::json!({"shape": 5, "curve": -4.0})),
    };
    let mut curve = Points::new(vec![shaped.clone()]);
    let before = curve
        .current(&payload(&curve.state()))
        .expect("a curve states itself");

    let flat = payload(&PointsIntent::SetPoints {
        points: vec![Point {
            at: 0.0,
            value: 0.0,
            data: Opaque::default(),
        }],
    });
    assert!(curve.apply(&flat).applied);
    assert!(curve.apply(&before).applied, "and the inverse goes back on");
    assert_eq!(curve.0, vec![shaped], "with the shape it was drawn with");
}

#[test]
fn a_point_with_nothing_to_say_writes_no_field() {
    let bare = serde_json::to_value(Point {
        at: 1.0,
        value: 0.5,
        data: Opaque::default(),
    })
    .expect("serializes");
    assert_eq!(bare, serde_json::json!({"at": 1.0, "value": 0.5}));
}
