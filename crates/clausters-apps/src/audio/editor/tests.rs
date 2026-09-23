use super::*;

fn call(editor: &mut AudioEditor, request: Value) -> Value {
    serde_json::from_str(&call_json(editor, &request.to_string())).unwrap()
}

/// A mono take of 100 frames in buffer 3 at 44.1 kHz, drawn through the join
/// in buffer 9, open in window 900 with its picture in widget 12, and buffers
/// 20 and 21 handed over for new takes.
fn opened(channels: u32) -> AudioEditor {
    let mut editor = new_json(
        &json!({"take": 3, "frames": 100, "channels": channels, "rate": 44100,
                "display": 9, "buffers": [20, 21]})
        .to_string(),
    )
    .unwrap();
    call(&mut editor, json!({"verb": "window", "widget": 12}));
    call(&mut editor, json!({"verb": "sync", "window": 900}));
    editor
}

/// Each part as `(buffer, start, end)`.
fn spans(editor: &mut AudioEditor) -> Vec<(u64, u64, u64)> {
    let parts = call(editor, json!({"verb": "parts"}))["parts"].clone();
    parts
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let r = &p["source"]["range"];
            (
                p["source"]["source"].as_u64().unwrap(),
                r["start"].as_u64().unwrap(),
                r["end"].as_u64().unwrap(),
            )
        })
        .collect()
}

/// The addresses a turn's steps send, and what they wait for, in order.
fn walk(steps: &Value) -> Vec<String> {
    steps
        .as_array()
        .unwrap()
        .iter()
        .map(|step| {
            if let Some(send) = step.get("send") {
                send["addr"].as_str().unwrap().to_string()
            } else if let Some(awaited) = step.get("await") {
                format!("await {}", awaited["command"].as_str().unwrap())
            } else {
                "sync".into()
            }
        })
        .collect()
}

fn args(steps: &Value, addr: &str) -> Vec<Value> {
    steps
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["send"]["addr"] == addr)
        .map(|s| s["send"]["args"].as_array().unwrap().clone())
        .unwrap_or_default()
}

fn event(editor: &mut AudioEditor, args: Value) -> Value {
    call(
        editor,
        json!({"verb": "event", "addr": "/gui_event", "version": 0, "args": args}),
    )
}

#[test]
fn opening_stitches_the_window_s_join_over_the_whole_take() {
    let mut editor = opened(1);
    let steps = call(&mut editor, json!({"verb": "open"}))["steps"].clone();
    assert_eq!(walk(&steps), ["/buffer_stitch", "await /buffer_stitch"]);
    let stitched = args(&steps, "/buffer_stitch");
    assert_eq!(stitched[0], json!({"i": 9}), "the display buffer");
    assert_eq!(stitched[3], json!({"i": 3}), "reads the take");
    assert_eq!(stitched[5], json!({"i": 100}), "all of it");
    let def = call(&mut editor, json!({"verb": "window", "widget": 12}));
    assert_eq!(def["children"][0]["buffer"], 9, "the window draws the join");
}

#[test]
fn a_stroke_is_a_new_take_the_size_of_the_stroke_spliced_over_its_frames() {
    let mut editor = opened(1);
    let out = event(
        &mut editor,
        json!([12, 1, 0, "draw", 0, 40, [0.5, -0.5], [0.0, 0.0]]),
    );
    assert_eq!(out["turn"], "route");
    assert_eq!(out["answer"]["seq"], 1);
    let steps = &out["steps"];
    assert_eq!(
        walk(steps),
        [
            "/buffer_alloc",
            "sync",
            "/buffer_setRange",
            "await /buffer_setRange",
            "/buffer_stitch",
            "await /buffer_stitch"
        ]
    );
    assert_eq!(
        args(steps, "/buffer_alloc"),
        [
            json!({"i": 20}),
            json!({"i": 2}),
            json!({"i": 1}),
            json!({"f": 44100.0})
        ],
        "the first buffer handed over, two frames, at the take's rate"
    );
    assert_eq!(spans(&mut editor), [(3, 0, 40), (20, 0, 2), (3, 42, 100)]);

    let leg = &out["record"]["legs"][0];
    assert_eq!(leg["forward"]["edit"]["intent"], "parts");
    assert_eq!(
        leg["backward"]["parts"].as_array().unwrap().len(),
        1,
        "the list before"
    );
    assert_eq!(leg["holdsForward"], json!([3, 20]));
    assert_eq!(leg["holdsBackward"], json!([3]));
}

#[test]
fn a_stroke_over_a_wider_take_starts_as_the_frames_it_was_drawn_over() {
    let mut editor = opened(2);
    let out = event(&mut editor, json!([12, 1, 0, "sample", 1, 10, 0.5, 0.0]));
    let steps = &out["steps"];
    assert_eq!(
        walk(steps)[..5],
        [
            "/buffer_alloc",
            "sync",
            "/buffer_gen",
            "await /buffer_gen",
            "/buffer_setRangeChannel"
        ]
    );
    assert_eq!(
        args(steps, "/buffer_gen"),
        [
            json!({"i": 20}),
            json!({"s": "copy"}),
            json!({"i": 0}),
            json!({"i": 9}),
            json!({"i": 20}),
            json!({"i": 2})
        ],
        "copied out of the join, flat: frame 10 of a stereo take is sample 20"
    );
    let write = args(steps, "/buffer_setRangeChannel");
    assert_eq!(
        write[..3],
        [json!({"i": 20}), json!({"i": 1}), json!({"i": 0})]
    );
}

#[test]
fn a_stroke_with_no_buffer_to_write_into_is_refused_and_reread() {
    let mut editor = new_json(r#"{"take": 3, "frames": 100, "display": 9}"#).unwrap();
    call(&mut editor, json!({"verb": "window", "widget": 12}));
    let out = event(&mut editor, json!([12, 1, 0, "draw", 0, 0, [0.5], [0.0]]));
    assert!(out.get("steps").is_none());
    assert!(
        out["answer"]["reason"]
            .as_str()
            .unwrap()
            .contains("no buffer")
    );
    assert_eq!(
        out["answer"]["corrections"],
        json!([{"widget": 12, "props": {"reload": 1}}])
    );
    assert_eq!(spans(&mut editor), [(3, 0, 100)]);
}

#[test]
fn a_cut_takes_the_span_out_and_writes_no_samples() {
    let mut editor = opened(1);
    let out = event(&mut editor, json!([12, 1, 0, "cut", 10.0, 20.0]));
    assert_eq!(
        walk(&out["steps"]),
        ["/buffer_stitch", "await /buffer_stitch"]
    );
    assert_eq!(spans(&mut editor), [(3, 0, 10), (3, 30, 100)]);
    assert_eq!(out["record"]["label"], "cut");
    assert_eq!(
        out["answer"]["corrections"],
        json!([{"widget": 12, "props": {"reload": 1}}]),
        "the join is replaced whole, so the picture reads it again"
    );
    assert_eq!(call(&mut editor, json!({"verb": "parts"}))["frames"], 80);

    let all = event(&mut editor, json!([12, 2, 1, "cut", 0.0, 80.0]));
    assert!(
        all["answer"]["reason"]
            .as_str()
            .unwrap()
            .contains("nothing")
    );
}

#[test]
fn a_paste_is_a_new_take_holding_the_block_put_in_at_the_position() {
    let mut editor = opened(1);
    let clip = Clipboard::samples(1, 3, 44100.0, 0);
    let doc = serde_json::to_string(&clip).unwrap();
    let out = event(
        &mut editor,
        json!([12, 1, 0, "paste", 50.0, "samples", doc, [0.1, 0.2, 0.3]]),
    );
    assert_eq!(
        walk(&out["steps"])[..4],
        [
            "/buffer_alloc",
            "sync",
            "/buffer_setRange",
            "await /buffer_setRange"
        ]
    );
    assert_eq!(spans(&mut editor), [(3, 0, 50), (20, 0, 3), (3, 50, 100)]);
    assert_eq!(out["record"]["label"], "paste");
}

#[test]
fn a_block_of_another_width_or_rate_is_refused() {
    let mut editor = opened(1);
    let wide = serde_json::to_string(&Clipboard::samples(2, 1, 44100.0, 0)).unwrap();
    let out = event(
        &mut editor,
        json!([12, 1, 0, "paste", 0.0, "samples", wide, [0.1, 0.2]]),
    );
    assert!(
        out["answer"]["reason"]
            .as_str()
            .unwrap()
            .contains("2 channels")
    );
    let fast = serde_json::to_string(&Clipboard::samples(1, 1, 48000.0, 0)).unwrap();
    let out = event(
        &mut editor,
        json!([12, 2, 0, "paste", 0.0, "samples", fast, [0.1]]),
    );
    assert!(out["answer"]["reason"].as_str().unwrap().contains("Hz"));
    assert_eq!(spans(&mut editor), [(3, 0, 100)]);
}

#[test]
fn a_list_the_history_hands_back_is_the_take_again() {
    let mut editor = opened(1);
    let out = event(&mut editor, json!([12, 1, 0, "cut", 10.0, 20.0]));
    let before = out["record"]["legs"][0]["backward"].clone();
    let steps = call(&mut editor, json!({"verb": "apply", "payload": before}))["steps"].clone();
    assert_eq!(walk(&steps), ["/buffer_stitch", "await /buffer_stitch"]);
    assert_eq!(spans(&mut editor), [(3, 0, 100)]);
    assert_eq!(
        call(
            &mut editor,
            json!({"verb": "apply", "payload": {"intent": "write"}})
        ),
        json!({})
    );
}

#[test]
fn a_mix_is_a_new_take_over_the_frames_the_block_lands_on() {
    let mut editor = opened(1);
    let doc = serde_json::to_string(&Clipboard::samples(1, 3, 44100.0, 0)).unwrap();
    let out = event(
        &mut editor,
        json!([12, 1, 0, "mix", 98.0, "samples", doc, [0.1, 0.2, 0.3]]),
    );
    let steps = &out["steps"];
    assert_eq!(
        walk(steps),
        [
            "/buffer_alloc",
            "sync",
            "/buffer_gen",
            "await /buffer_gen",
            "/buffer_alloc",
            "sync",
            "/buffer_setRange",
            "await /buffer_setRange",
            "/buffer_mix",
            "await /buffer_mix",
            "/buffer_free",
            "await /buffer_free",
            "/buffer_stitch",
            "await /buffer_stitch"
        ]
    );
    assert_eq!(
        args(steps, "/buffer_mix"),
        [
            json!({"i": 20}),
            json!({"i": 0}),
            json!({"i": 21}),
            json!({"i": 0}),
            json!({"i": 2}),
            json!({"f": 1.0})
        ],
        "two frames: the block runs past the end of the take and is mixed as far as it goes"
    );
    assert_eq!(spans(&mut editor), [(3, 0, 98), (20, 0, 2)]);
    assert_eq!(out["record"]["label"], "mix");

    // The scratch buffer came back: the next mix takes 21 as its take.
    let again = event(
        &mut editor,
        json!([12, 2, 1, "mix", 0.0, "samples", doc, [0.1, 0.2, 0.3]]),
    );
    assert!(
        again["answer"]["reason"]
            .as_str()
            .unwrap()
            .contains("two buffers"),
        "one buffer left, and a mix needs two"
    );
}
