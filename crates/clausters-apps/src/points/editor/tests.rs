use serde_json::json;

use super::*;
use crate::points::{Rules, shared_of};

/// A ramp up and down over two seconds, at a rate of 100 samples a second.
fn shared() -> Shared {
    shared_of(&json!({
        "points": [0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0, 2.0, 0.0, 1.0, 0.0],
        "name": "amp",
    }))
}

fn editor(curve: Shared, request: &str) -> PointsEditor {
    let mut editor = new_json(curve, request);
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

fn quads(values: &[f64]) -> Vec<Value> {
    values.iter().map(|v| json!(v)).collect()
}

#[test]
fn the_window_draws_the_curve_on_the_axis_the_caller_declared() {
    let mut e = editor(shared(), r#"{"rate": 100.0, "min": 0.0, "max": 1.0}"#);
    let tree = e.window(40);
    let curve = &tree["children"][0];
    assert_eq!(curve["type"], "curve");
    assert_eq!(curve["id"], 40);
    assert_eq!(curve["label"], "amp");
    assert_eq!(curve["points"].as_array().unwrap().len(), 12);
    assert_eq!(curve["axes"]["y"], json!({"min": 0.0, "max": 1.0}));
}

/// The curve is edited in place, the outcome hands its holder the points to
/// write back, and the entry puts it back.
#[test]
fn a_gesture_edits_the_shared_curve_and_leaves_its_inverse() {
    let curve = shared();
    let mut e = editor(curve.clone(), r#"{"rate": 100.0}"#);
    let drawn = [0.0, 0.0, 1.0, 0.0, 1.0, 0.5, 5.0, 2.0, 2.0, 0.0, 1.0, 0.0];
    let out = e.event(&gesture("points", quads(&drawn)), 1);
    assert_eq!(out.turn, Kind::Route);
    assert!(out.changed && out.version == 2);
    assert_eq!(out.points.as_deref(), Some(&drawn[..]));
    assert_eq!(curve.lock().unwrap().points[1].value, 0.5);
    let record = out.record.expect("an entry");
    assert_eq!(record.legs[0].key, POINTS);
    let back = e
        .apply(&record.legs[0].backward)
        .expect("it changed the curve");
    assert_eq!(back[5], 1.0, "the peak is back");
    assert_eq!(back[6], 1.0, "and its segment is straight again");
}

/// What the widget sent back unchanged is not an edit.
#[test]
fn a_report_of_the_curve_as_it_is_records_nothing() {
    let mut e = editor(shared(), r#"{"rate": 100.0}"#);
    let same = [0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0, 2.0, 0.0, 1.0, 0.0];
    let out = e.event(&gesture("points", quads(&same)), 1);
    assert!(!out.changed && out.record.is_none() && out.points.is_none());
}

/// A sweep leaves a time range in the curve's seconds and, with height, a
/// value band; the points inside both are what is selected.
#[test]
fn a_sweep_selects_the_points_inside_it() {
    let mut e = editor(shared(), r#"{"rate": 100.0}"#);
    let out = e.event(&gesture("selection", quads(&[50.0, 150.0])), 1);
    assert_eq!(out.span, Some(json!([0.5, 2.0])));
    assert_eq!(e.selected(), vec![1, 2]);

    e.event(&gesture("selection", quads(&[0.0, 200.0, 0.5, 1.0])), 1);
    assert_eq!(e.selected(), vec![1], "only the peak is in the band");

    let out = e.event(&gesture("selection", quads(&[0.0, 0.0])), 1);
    assert_eq!(out.span, Some(Value::Null));
    assert!(e.selected().is_empty());
}

/// A span set by a caller is drawn as the band a sweep leaves, in samples.
#[test]
fn a_span_set_from_the_door_is_drawn_and_selects() {
    let mut e = editor(shared(), r#"{"rate": 100.0}"#);
    let answer = call_json(&mut e, r#"{"verb": "span", "span": [0.0, 1.0]}"#);
    assert_eq!(answer, r#"{"span":[0.0,1.0]}"#);
    let props: Value =
        serde_json::from_str(&call_json(&mut e, r#"{"verb": "props", "widget": 40}"#)).unwrap();
    assert_eq!(
        (props["sel_start"].clone(), props["sel_len"].clone()),
        (json!(0.0), json!(100.0))
    );
    let selected = call_json(&mut e, r#"{"verb": "selected"}"#);
    assert_eq!(selected, r#"{"points":[0,1]}"#);
    call_json(&mut e, r#"{"verb": "span", "span": null}"#);
    assert_eq!(
        call_json(&mut e, r#"{"verb": "selected"}"#),
        r#"{"points":[]}"#
    );
}

/// The holder's curve, changed by a script between turns, is what the next
/// turn reads.
#[test]
fn a_sync_takes_the_curve_as_its_holder_has_it() {
    let curve = shared();
    let mut e = editor(curve.clone(), r#"{"rate": 100.0}"#);
    call_json(
        &mut e,
        r#"{"verb": "sync", "points": [0.0, 0.2, 1.0, 0.0, 3.0, 0.4, 1.0, 0.0]}"#,
    );
    assert_eq!(curve.lock().unwrap().points.len(), 2);
    let state: Value = serde_json::from_str(&call_json(&mut e, r#"{"verb": "state"}"#)).unwrap();
    assert_eq!(
        state["points"],
        json!([0.0, 0.2, 1.0, 0.0, 3.0, 0.4, 1.0, 0.0])
    );
}

/// A curve with no range is drawn on an axis that grows to hold a point
/// dragged past it, and never narrows.
#[test]
fn with_no_range_the_axis_grows_with_the_curve_and_holds() {
    let mut e = editor(shared(), r#"{"rate": 100.0}"#);
    assert_eq!(e.rules(), Rules::default());
    let out = e.event(
        &gesture("points", quads(&[0.0, 0.0, 1.0, 0.0, 1.0, 3.0, 1.0, 0.0])),
        1,
    );
    let Some(Answer::Push { corrections, .. }) = out.answer else {
        panic!("an answer with the curve");
    };
    let grown = corrections[0].props["max"].as_f64().unwrap();
    assert!(grown >= 3.0);
    e.event(
        &gesture("points", quads(&[0.0, 0.0, 1.0, 0.0, 1.0, 0.5, 1.0, 0.0])),
        2,
    );
    let props: Value =
        serde_json::from_str(&call_json(&mut e, r#"{"verb": "props", "widget": 40}"#)).unwrap();
    assert_eq!(props["max"].as_f64().unwrap(), grown);
}

/// **A declared range is a rule**: the axis is the range and holds, and a
/// point reported outside it is kept inside -- a normalized envelope, time and
/// value both from 0 to 1.
#[test]
fn a_declared_range_keeps_the_curve_inside_it() {
    let curve = shared();
    let mut e = editor(
        curve.clone(),
        r#"{"rate": 100.0, "min": 0.0, "max": 1.0, "start": 0.0, "end": 1.0}"#,
    );
    let out = e.event(
        &gesture("points", quads(&[0.0, -0.5, 1.0, 0.0, 1.5, 3.0, 1.0, 0.0])),
        1,
    );
    assert_eq!(
        out.points.as_deref(),
        Some(&[0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0][..])
    );
    let Some(Answer::Push { corrections, .. }) = out.answer else {
        panic!("an answer with the curve");
    };
    let props = &corrections[0].props;
    assert_eq!(
        (props["min"].clone(), props["max"].clone()),
        (json!(0.0), json!(1.0))
    );
    assert_eq!(
        props["duration"],
        json!(1.0),
        "the time range is the span drawn"
    );
    assert_eq!(
        call_json(&mut e, r#"{"verb": "rules"}"#),
        r#"{"time":[0.0,1.0],"values":[0.0,1.0]}"#
    );
}

/// **An automation's range is its parameter's**, read by the rule the roll
/// and the multitrack read: a CC goes from 0 to 127 with nothing declared.
#[test]
fn an_automation_is_kept_in_the_range_of_what_it_automates() {
    let curve = shared_of(&json!({
        "points": [0.0, 10.0, 1.0, 0.0, 1.0, 100.0, 1.0, 0.0],
        "target": {"cc": 74},
    }));
    let mut e = editor(curve, r#"{"rate": 100.0}"#);
    assert_eq!(e.rules().values, Some((0.0, 127.0)));
    let out = e.event(
        &gesture(
            "points",
            quads(&[0.0, 10.0, 1.0, 0.0, 1.0, 200.0, 1.0, 0.0]),
        ),
        1,
    );
    assert_eq!(out.points.unwrap()[5], 127.0);
}

/// **The rules are in sight by default**: the window draws the time ruler,
/// in the curve's own seconds, and the value ruler.
#[test]
fn the_window_shows_its_rulers() {
    let mut e = editor(shared(), r#"{"rate": 100.0}"#);
    let tree = e.window(40);
    let curve = &tree["children"][0];
    assert_eq!(curve["ruler"], "time");
    assert_eq!(curve["ruler_y"], "value");
    assert_eq!(curve["sample_rate"], 1.0);
}
