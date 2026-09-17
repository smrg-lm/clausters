use serde_json::json;

use super::*;

#[test]
fn a_take_draws_as_a_trace_with_the_three_gestures() {
    let props = waveform(&Waveform {
        buffer: Some(3),
        channels: Some(2),
        measure: "peak rms".into(),
        ruler: "time".into(),
        sample_rate: 48_000.0,
        tempo: Some(1.0),
        label: "take".into(),
        ..Waveform::default()
    });
    assert_eq!(props["type"], "signal");
    assert_eq!(props["view"], "trace");
    assert_eq!(props["buffer"], 3);
    assert_eq!(props["channels"], 2);
    assert_eq!(props["measure"], "peak rms");
    assert_eq!(props["axes"]["x"]["unit"], "time");
    assert_eq!(props["axes"]["x"]["sample_rate"], 48_000.0);
    assert_eq!(props["gestures"]["alt"], "draw");
    assert_eq!(props["gestures"]["ctrl"], "sample");
    assert!(
        props.get("h").is_none(),
        "elastic unless a height is stated"
    );
    assert!(props.get("playhead_at").is_none());
}

#[test]
fn a_pane_beside_a_document_states_its_height_and_its_anchor() {
    let props = waveform(&Waveform {
        buffer: Some(1),
        ruler: "samples".into(),
        sample_rate: 48_000.0,
        height: Some(96.0),
        playhead_at: Some(0.0),
        ..Waveform::default()
    });
    assert_eq!(props["axes"]["x"]["unit"], "samples");
    assert_eq!(props["h"], 96.0);
    assert_eq!(props["axes"]["x"]["playhead_at"], 0.0);
    assert!(
        props.get("measure").is_none(),
        "nothing said is nothing sent"
    );
}

#[test]
fn a_channel_count_is_never_zero() {
    let props = waveform(&Waveform {
        channels: Some(0),
        ..Waveform::default()
    });
    assert_eq!(props["channels"], 1);
}

#[test]
fn a_curve_draws_on_the_axis_it_is_handed() {
    let props = bpf(&Curve {
        points: vec![0.0, 1.0, 1.0, 0.0, 2.0, 3.0, 1.0, 0.0],
        min: 0.0,
        max: 4.0,
        duration: 2.0,
        label: "cutoff".into(),
    });
    assert_eq!(props["type"], "curve");
    assert_eq!(props["axes"]["y"]["min"], 0.0);
    assert_eq!(props["axes"]["y"]["max"], 4.0);
    assert_eq!(props["duration"], 2.0);
    assert_eq!(props["points"][5], 3.0);
}

#[test]
fn a_curve_with_no_span_lets_the_widget_fit_it() {
    let props = bpf(&Curve {
        points: vec![0.0, 1.0, 1.0, 0.0],
        max: 1.0,
        ..Curve::default()
    });
    assert!(props.get("duration").is_none());
    assert!(props.get("label").is_none());
}

#[test]
fn a_roll_fits_its_pitch_window_to_the_notes_it_holds() {
    let props = pianoroll(&Roll {
        notes: vec![0.0, 1.0, 60.0, 100.0, 0.0, 1.0, 1.0, 72.0, 100.0, 0.0],
        tempo: 1.0,
        sample_rate: 48_000.0,
        ..Roll::default()
    });
    assert_eq!(props["type"], "notes");
    assert_eq!(props["axes"]["x"]["unit"], "beats");
    assert_eq!(props["axes"]["y"]["min"], 56.0, "the lowest, less the pad");
    assert_eq!(props["axes"]["y"]["max"], 76.0, "the highest, plus it");
    assert!(props.get("notes_editable").is_none(), "editable by default");
}

#[test]
fn a_rolls_ruler_reads_the_timelines_map_when_it_is_given_one() {
    let map = r#"[{"beats":0.0,"tempo":2.0}]"#;
    let props = props(
        "pianoroll",
        &json!({"notes": [], "osc": [], "ruler": "beats", "tempo_map": map,
                "sample_rate": 48_000.0}),
    )
    .expect("a roll without a scalar tempo still reads");
    assert_eq!(props["axes"]["x"]["tempo_map"], map);
    assert!(
        props["axes"]["x"].get("tempo").is_none(),
        "no tempo was stated"
    );
}

#[test]
fn a_roll_written_high_still_shows_where_the_ordinary_range_was() {
    // One note is its own window, padded: the piece is what is drawn.
    assert_eq!(pitch_window(&[0.0, 1.0, 60.0, 100.0, 0.0]), (56.0, 64.0));
    // Written two octaves up, the bottom is pulled back down to the fallback's
    // ceiling, so the eye keeps a reference the notes do not give it.
    assert_eq!(pitch_window(&[0.0, 1.0, 96.0, 100.0, 0.0]), (84.0, 100.0));
    assert_eq!(
        pitch_window(&[]),
        (PITCH_FLOOR, PITCH_CEIL),
        "nothing to fit"
    );
}

#[test]
fn a_roll_over_a_generator_refuses_the_press_before_it_happens() {
    let props = pianoroll(&Roll {
        notes: vec![0.0, 1.0, 60.0, 100.0, 0.0],
        editable: false,
        ..Roll::default()
    });
    assert_eq!(props["notes_editable"], false);
}

#[test]
fn an_empty_roll_states_no_window_and_no_notes() {
    let props = pianoroll(&Roll::default());
    assert!(props.get("notes").is_none());
    assert!(
        props.get("min").is_none(),
        "nothing to fit is nothing to say"
    );
}

#[test]
fn the_door_names_the_view_and_a_kind_it_does_not_draw_is_nothing() {
    let asked = props("bpf", &json!({"points": [0.0, 1.0, 1.0, 0.0], "max": 1.0})).unwrap();
    assert_eq!(asked["type"], "curve");
    let roll = props("pianoroll", &json!({"notes": [0.0, 1.0, 60.0, 100.0, 0.0]})).unwrap();
    assert!(
        roll.get("notes_editable").is_none(),
        "a caller that says nothing means yes"
    );
    assert!(
        props("multitrack", &json!({})).is_none(),
        "the piece has its own door"
    );
    assert!(props("waveform", &json!(["not", "facts"])).is_none());
}
