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
fn opening_copies_the_take_and_stitches_the_window_s_join_over_the_copy() {
    let mut editor = opened(1);
    let steps = call(&mut editor, json!({"verb": "open"}))["steps"].clone();
    assert_eq!(
        walk(&steps),
        [
            "/buffer_alloc",
            "sync",
            "/buffer_gen",
            "await /buffer_gen",
            "/buffer_stitch",
            "await /buffer_stitch"
        ]
    );
    assert_eq!(
        args(&steps, "/buffer_gen"),
        [
            json!({"i": 20}),
            json!({"s": "copy"}),
            json!({"i": 0}),
            json!({"i": 3}),
            json!({"i": 0}),
            json!({"i": 100})
        ],
        "a private copy of the buffer it was opened over"
    );
    let stitched = args(&steps, "/buffer_stitch");
    assert_eq!(stitched[0], json!({"i": 9}), "the display buffer");
    assert_eq!(
        stitched[3],
        json!({"i": 20}),
        "reads the copy, never the take"
    );
    assert_eq!(spans(&mut editor), [(20, 0, 100)]);
    let again = call(&mut editor, json!({"verb": "open"}))["steps"].clone();
    assert_eq!(
        walk(&again),
        ["/buffer_stitch", "await /buffer_stitch"],
        "copied once"
    );
    let def = call(&mut editor, json!({"verb": "window", "widget": 12}));
    assert_eq!(def["children"][1]["buffer"], 9, "the window draws the join");

    let bare = &mut new_json(r#"{"take": 3, "frames": 100, "display": 9}"#).unwrap();
    assert!(
        call(bare, json!({"verb": "open"}))["error"].is_string(),
        "no buffer for the copy"
    );
}

#[test]
fn a_save_over_a_buffer_rewrites_it_at_the_take_s_length() {
    let mut editor = opened(1);
    call(&mut editor, json!({"verb": "open"}));
    call(&mut editor, json!({"verb": "sync", "buffers": [30, 31]}));
    event(&mut editor, json!([12, 1, 0, "cut", 10.0, 20.0]));
    let saved = call(&mut editor, json!({"verb": "save"}));
    assert_eq!(saved["buffer"], 3, "over the buffer it was opened from");
    let steps = &saved["steps"];
    assert_eq!(
        walk(steps),
        ["/buffer_alloc", "sync", "/buffer_gen", "await /buffer_gen"]
    );
    assert_eq!(
        args(steps, "/buffer_alloc")[..2],
        [json!({"i": 3}), json!({"i": 80})],
        "rewritten whole, at the cut take's length"
    );
    assert_eq!(
        args(steps, "/buffer_gen")[3],
        json!({"i": 9}),
        "read out of the join"
    );

    let moved = call(&mut editor, json!({"verb": "save", "buffer": 40}));
    assert_eq!(moved["buffer"], 40, "a save-as into a new buffer");
    assert_eq!(call(&mut editor, json!({"verb": "save"}))["buffer"], 40);
    let refused = call(&mut editor, json!({"verb": "save", "buffer": 20}));
    assert!(
        refused["error"].as_str().unwrap().contains("reads"),
        "never into a buffer the list reads"
    );
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
        json!([
            {"widget": 12, "props": {"reload": 1}},
            {"widget": 12, "props": {"cursor": 10, "sel_start": 10, "sel_len": 0}}
        ]),
        "the join is replaced whole, so the picture reads it again; and the \
         selection is gone, the cursor where the span was"
    );
    assert_eq!(out["selection"]["len"], 0.0);
    assert_eq!(out["cue"], 10);
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
    assert_eq!(spans(&mut editor), [(3, 0, 100)]);
    // a mix is a sum taken sample for sample, so it is one rate
    let fast = serde_json::to_string(&Clipboard::samples(1, 1, 48000.0, 0)).unwrap();
    let out = event(
        &mut editor,
        json!([12, 2, 0, "mix", 0.0, "samples", fast, [0.1]]),
    );
    assert!(out["answer"]["reason"].as_str().unwrap().contains("Hz"));
    assert_eq!(spans(&mut editor), [(3, 0, 100)]);
}

/// **A block at another rate pastes as a take at its own rate**, which the
/// list reads at the ratio: nothing is converted on the way in, and a second
/// of the block is a second of the take.
#[test]
fn a_block_at_another_rate_pastes_at_its_own_rate() {
    let mut editor = opened(1);
    let fast = serde_json::to_string(&Clipboard::samples(1, 480, 48000.0, 0)).unwrap();
    let block: Vec<f64> = vec![0.25; 480];
    let out = event(
        &mut editor,
        json!([12, 1, 0, "paste", 50.0, "samples", fast, block]),
    );
    let steps = out["steps"].as_array().unwrap();
    let alloc = steps
        .iter()
        .find_map(|s| s.get("send").filter(|m| m["addr"] == "/buffer_alloc"))
        .unwrap();
    assert_eq!(
        alloc["args"][3],
        json!({"f": 48000.0}),
        "the take keeps the block's rate"
    );
    assert_eq!(spans(&mut editor), [(3, 0, 50), (20, 0, 480), (3, 50, 100)]);
    // 480 frames at 48 kHz are 441 of the take at 44.1 kHz
    assert_eq!(call(&mut editor, json!({"verb": "parts"}))["frames"], 541);
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

#[test]
fn a_save_writes_the_join_over_the_file_and_a_save_as_moves_it() {
    let mut editor = opened(1);
    call(&mut editor, json!({"verb": "sync", "path": "/takes/a.wav"}));
    let saved = call(&mut editor, json!({"verb": "save"}));
    assert_eq!(saved["path"], "/takes/a.wav");
    assert_eq!(
        args(&saved["steps"], "/buffer_write"),
        [
            json!({"i": 9}),
            json!({"s": "/takes/a.wav"}),
            json!({"s": "wav"}),
            json!({"s": "float"})
        ],
        "the join, over the file it was read from"
    );
    let moved = call(
        &mut editor,
        json!({"verb": "save", "path": "/takes/b.wav", "format": "int24"}),
    );
    assert_eq!(moved["path"], "/takes/b.wav");
    assert_eq!(
        call(&mut editor, json!({"verb": "save"}))["path"],
        "/takes/b.wav"
    );
    assert!(call(&mut editor, json!({"verb": "save", "format": "mp3"}))["error"].is_string());
}

#[test]
fn ctrl_s_over_the_window_is_a_save() {
    let mut editor = opened(1);
    call(&mut editor, json!({"verb": "sync", "path": "/takes/a.wav"}));
    let out = event(&mut editor, json!([900, 5, 0, "save"]));
    assert_eq!(
        walk(&out["steps"]),
        ["/buffer_write", "await /buffer_write"]
    );
    assert_eq!(out["answer"]["seq"], 5);
    assert!(out.get("record").is_none(), "a save is no edit");
}

/// **The space bar over the window is the application's play**: with nothing
/// placed the take plays whole and goes back to its start; a placed cursor is
/// where it starts and goes back to; a selection plays its span; and the loop
/// switch the host sends beside the verb makes it repeat.
#[test]
fn the_space_bar_asks_the_playback_for_the_pass_the_view_says() {
    let mut editor = opened(1);
    let whole = event(&mut editor, json!([900, 1, 0, "play", 0]));
    assert_eq!(
        whole["play"],
        json!({"start": 0, "pass": {"kind": "until", "end": 100, "back": 0}, "back": 0})
    );
    let placed = event(&mut editor, json!([12, 2, 0, "locate", 40]));
    assert_eq!(placed["cue"], 40, "the playback is cued on the cursor");
    let from = event(&mut editor, json!([900, 3, 0, "play", 0]));
    assert_eq!(from["play"]["start"], 40);
    assert_eq!(from["play"]["back"], 40);
    event(&mut editor, json!([12, 4, 0, "selection", 10, 20]));
    let looped = event(&mut editor, json!([900, 5, 0, "play", 1]));
    assert_eq!(
        looped["play"],
        json!({"start": 10, "pass": {"kind": "loop", "from": 10, "to": 30}, "back": 40})
    );
}

/// **The caller sets what a hand would**: a span in seconds is the frames a
/// sweep leaves, drawn as its band and played by the space bar; a cursor is
/// where a stop goes back to; and the play is the one the space bar asks for.
#[test]
fn a_span_and_a_cursor_set_by_the_caller_are_what_the_space_bar_plays() {
    let mut editor = opened(1);
    let band = call(
        &mut editor,
        json!({"verb": "span", "span": [10.0 / 44100.0, 30.0 / 44100.0]}),
    );
    assert_eq!(band, json!({"sel_start": 10, "sel_len": 20}));
    let cue = call(&mut editor, json!({"verb": "locate", "at": 40.0 / 44100.0}));
    assert_eq!(cue["cue"], 40);
    let play = call(&mut editor, json!({"verb": "play", "looping": true}));
    assert_eq!(
        play,
        json!({"start": 10, "pass": {"kind": "loop", "from": 10, "to": 30}, "back": 40})
    );
    let none = call(&mut editor, json!({"verb": "span", "span": null}));
    assert_eq!(
        none,
        json!({"sel_start": 0, "sel_len": 0}),
        "no span, no band"
    );
    let whole = call(&mut editor, json!({"verb": "play", "looping": false}));
    assert_eq!(whole["start"], 40, "from the cursor");
}

/// **`L` over the window changes the pass in progress**: how it ends now,
/// read off the view as a play's is, and no play.
#[test]
fn the_loop_key_asks_the_pass_in_progress_to_follow() {
    let mut editor = opened(1);
    event(&mut editor, json!([12, 2, 0, "selection", 10, 20]));
    let looped = event(&mut editor, json!([900, 3, 0, "loop", 1]));
    assert!(looped.get("play").is_none(), "no play");
    assert_eq!(
        looped["pass"],
        json!({"kind": "loop", "from": 10, "to": 30})
    );
}

/// **A selection redrawn is the span a loop in progress follows**: every
/// move of the sweep says the span, the locate it is let go with says to put
/// the head into it, and a click -- which empties the selection at its press
/// -- gives the loop the whole take back and places nothing.
#[test]
fn a_redrawn_selection_is_the_span_a_loop_in_progress_follows() {
    let mut editor = opened(1);
    let pressed = event(&mut editor, json!([12, 1, 0, "selection", 10, 0]));
    assert_eq!(
        pressed["follow"],
        json!({"from": 0, "to": 100, "place": false}),
        "no selection: the take"
    );
    let swept = event(&mut editor, json!([12, 2, 0, "selection", 10, 20]));
    assert_eq!(
        swept["follow"],
        json!({"from": 10, "to": 30, "place": false})
    );
    let let_go = event(&mut editor, json!([12, 3, 0, "locate", 10]));
    assert_eq!(
        let_go["follow"],
        json!({"from": 10, "to": 30, "place": true})
    );
    // A cursor placed anywhere else is a mark, and the loop is left alone.
    let marked = event(&mut editor, json!([12, 4, 0, "locate", 60]));
    assert!(marked.get("follow").is_none());
    event(&mut editor, json!([12, 5, 0, "selection", 60, 0]));
    let clicked = event(&mut editor, json!([12, 6, 0, "locate", 60]));
    assert!(clicked.get("follow").is_none(), "a click places no head");
}

/// **A sweep's start and the locate it is let go with are one frame**,
/// however far into a take: both reach the editor as doubles, so a cursor a
/// frame away from the selection's start is a mark and not the sweep's end.
#[test]
fn a_selection_far_into_a_take_is_matched_to_the_frame() {
    // An hour at 48 kHz, and an odd frame in it.
    let mut editor = new_json(
        &json!({"take": 3, "frames": 200_000_000_u64, "channels": 1, "rate": 48000,
                "display": 9, "buffers": [20, 21]})
        .to_string(),
    )
    .unwrap();
    call(&mut editor, json!({"verb": "window", "widget": 12}));
    let far = 172_800_001_u64;
    event(&mut editor, json!([12, 1, 0, "selection", far, 20]));
    let beside = event(&mut editor, json!([12, 2, 0, "locate", far + 1]));
    assert!(beside.get("follow").is_none(), "a frame away is elsewhere");
    let let_go = event(&mut editor, json!([12, 3, 0, "locate", far]));
    assert_eq!(let_go["follow"]["place"], json!(true));
}

/// **The level meter stands beside the take** once a playback says where it
/// writes, and the window says the space bar is its own.
#[test]
fn the_window_carries_the_meter_and_plays_its_own_take() {
    let mut editor = opened(2);
    let bare = call(
        &mut editor,
        json!({"verb": "window", "widget": 12, "meter": 13}),
    );
    assert_eq!(
        bare["children"].as_array().unwrap().len(),
        2,
        "the toolbar and the take: nothing measures it yet"
    );
    assert_eq!(bare["plays"], true);
    call(
        &mut editor,
        json!({"verb": "sync", "meters": {"bus": 40, "channels": 2}}),
    );
    let window = call(
        &mut editor,
        json!({"verb": "window", "widget": 12, "meter": 13}),
    );
    let meter = &window["children"][1]["children"][1];
    assert_eq!(meter["type"], "meter");
    assert_eq!(meter["id"], 13);
    assert_eq!(meter["bus"], 40);
    assert_eq!(meter["rate"], "control");
    assert_eq!(meter["scale"], "db", "an amplitude, read in decibels");
    assert_eq!(meter["channels"], 2);
}

/// The audio editor opened with its dialogs numbered from 400, in the order
/// the crate names them.
fn with_dialogs(facts: Value) -> (AudioEditor, BTreeMap<String, i64>) {
    let mut editor = opened(2);
    call(&mut editor, facts);
    let names = call(&mut editor, json!({"verb": "dialogs"}))["dialogs"].clone();
    let ids: BTreeMap<String, i64> = names
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str().unwrap().to_string(), 400 + i as i64))
        .collect();
    let window = call(
        &mut editor,
        json!({"verb": "window", "widget": 12, "dialogs": ids}),
    );
    assert!(
        window.to_string().contains("\"Save as\""),
        "the window holds the dialogs' stack"
    );
    (editor, ids)
}

/// **A file not written yet asks where it goes**: Ctrl+S opens the save-as
/// form rather than writing, and its `OK` writes the join there -- the path
/// the file is from then on.
#[test]
fn an_untitled_file_is_saved_through_the_save_as_form() {
    let (mut editor, ids) = with_dialogs(json!({"verb": "sync", "untitled": true}));
    assert_eq!(
        call(&mut editor, json!({"verb": "file"}))["path"],
        Value::Null
    );
    let asked = event(&mut editor, json!([900, 1, 0, "save"]));
    assert!(asked.get("steps").is_none(), "nothing is written yet");
    let corrections = asked["answer"]["corrections"].to_string();
    assert!(
        corrections.contains("\"index\":2"),
        "the save-as page: {corrections}"
    );

    event(
        &mut editor,
        json!([ids["file:path"], 2, 0, "/takes/new.wav"]),
    );
    let saved = event(&mut editor, json!([ids["file:ok"], 3, 0, "click"]));
    assert_eq!(
        args(&saved["steps"], "/buffer_write")[1],
        json!({"s": "/takes/new.wav"})
    );
    assert!(
        saved["answer"]["corrections"]
            .to_string()
            .contains("\"index\":0")
    );
    let file = call(&mut editor, json!({"verb": "file"}));
    assert_eq!(file["path"], "/takes/new.wav");
    assert_eq!(file["untitled"], false);
    // and the next Ctrl+S writes there with no form
    let again = event(&mut editor, json!([900, 4, 0, "save"]));
    assert_eq!(
        walk(&again["steps"]),
        ["/buffer_write", "await /buffer_write"]
    );
}

/// A save-as with no path stays up and says why; Cancel takes it down.
#[test]
fn a_save_as_with_no_path_is_refused_and_cancel_closes_it() {
    let (mut editor, ids) = with_dialogs(json!({"verb": "sync", "path": "/takes/a.wav"}));
    let shown = event(&mut editor, json!([900, 1, 0, "save_as"]));
    assert!(
        shown["answer"]["corrections"]
            .to_string()
            .contains("/takes/a.wav")
    );
    event(&mut editor, json!([ids["file:path"], 2, 0, "  "]));
    let refused = event(&mut editor, json!([ids["file:ok"], 3, 0, "click"]));
    assert!(
        refused["answer"]["reason"]
            .as_str()
            .unwrap()
            .contains("path")
    );
    let cancelled = event(&mut editor, json!([ids["file:cancel"], 4, 0, "click"]));
    assert!(
        cancelled["answer"]["corrections"]
            .to_string()
            .contains("\"index\":0")
    );
    assert_eq!(
        call(&mut editor, json!({"verb": "file"}))["path"],
        "/takes/a.wav"
    );
}

/// **A new file is the client's to open**: the form opens on the window's
/// shape, and its `OK` names the shape asked for in the outcome.
#[test]
fn the_new_file_form_names_a_shape_for_the_client() {
    let (mut editor, ids) = with_dialogs(json!({"verb": "sync"}));
    let shown = event(&mut editor, json!([900, 1, 0, "new_file"]));
    let text = shown["answer"]["corrections"].to_string();
    assert!(
        text.contains("\"value\":\"2\"") && text.contains("44100"),
        "{text}"
    );
    event(&mut editor, json!([ids["new:channels"], 2, 0, "4"]));
    event(&mut editor, json!([ids["new:seconds"], 3, 0, "1.5"]));
    let made = event(&mut editor, json!([ids["new:ok"], 4, 0, "click"]));
    assert_eq!(
        made["newFile"],
        json!({"channels": 4, "frames": 66150, "rate": 44100.0})
    );
    event(&mut editor, json!([900, 5, 0, "new_file"]));
    event(&mut editor, json!([ids["new:seconds"], 6, 0, "long"]));
    let refused = event(&mut editor, json!([ids["new:ok"], 7, 0, "click"]));
    assert!(refused.get("newFile").is_none());
    assert!(refused["answer"]["reason"].is_string());
}

/// A window composed without the dialogs saves an untitled file nowhere and
/// says so, and opens no form.
#[test]
fn without_dialogs_an_untitled_save_is_refused() {
    let mut editor = opened(1);
    call(&mut editor, json!({"verb": "sync", "untitled": true}));
    let out = event(&mut editor, json!([900, 1, 0, "save"]));
    assert!(out["answer"]["reason"].as_str().unwrap().contains("path"));
    let form = event(&mut editor, json!([900, 2, 0, "new_file"]));
    assert!(
        form["answer"]["reason"]
            .as_str()
            .unwrap()
            .contains("dialogs")
    );
}

/// **Folding is the editor's view state**: a script's verb, a menu entry
/// carrying its channel and a press on a strip's name all change one list, and
/// each answers with the take widget's `folded` prop.
#[test]
fn channels_fold_solo_and_unfold_through_one_state() {
    let mut editor = opened(4);
    let folded = |out: &Value| out["answer"]["corrections"][0]["props"]["folded"].clone();
    let act = call(
        &mut editor,
        json!({"verb": "fold_channel", "version": 0, "channel": 2}),
    );
    assert_eq!(folded(&act), json!([2]));
    let solo = event(&mut editor, json!([900, 1, 0, "solo_channel:1"]));
    assert_eq!(folded(&solo), json!([0, 2, 3]));
    let unfold = event(&mut editor, json!([12, 2, 0, "unfold", 3]));
    assert_eq!(folded(&unfold), json!([0, 2]));
    assert_eq!(
        call(&mut editor, json!({"verb": "folded"}))["folded"],
        json!([0, 2])
    );
    let window = call(&mut editor, json!({"verb": "window", "widget": 12}));
    assert!(
        window.to_string().contains("\"folded\":[0,2]"),
        "the window draws it"
    );
    assert!(
        window.to_string().contains("solo_channel:3"),
        "a menu entry per channel"
    );
    let all = event(&mut editor, json!([900, 3, 0, "show_all_channels"]));
    assert_eq!(folded(&all), json!([]));
    let wrong = call(
        &mut editor,
        json!({"verb": "fold_channel", "version": 0, "channel": 9}),
    );
    assert!(
        wrong["answer"]["reason"]
            .as_str()
            .unwrap()
            .contains("channel 9")
    );
}

/// **A field's focus is not its text**: the host reports `"focus" 1` when a
/// field takes the keyboard, and the form keeps what was typed. A field that
/// holds the word `focus` is still a field.
#[test]
fn a_fields_focus_report_is_not_what_it_holds() {
    let (mut editor, ids) = with_dialogs(json!({"verb": "sync"}));
    event(&mut editor, json!([900, 1, 0, "new_file"]));
    event(&mut editor, json!([ids["new:channels"], 2, 0, "focus", 1]));
    event(&mut editor, json!([ids["new:channels"], 3, 0, "3"]));
    event(&mut editor, json!([ids["new:channels"], 4, 0, "focus", 0]));
    let made = event(&mut editor, json!([ids["new:ok"], 5, 0, "click"]));
    assert_eq!(made["newFile"]["channels"], 3);
    event(&mut editor, json!([900, 6, 0, "save_as"]));
    event(&mut editor, json!([ids["file:path"], 7, 0, "focus"]));
    let saved = event(&mut editor, json!([ids["file:ok"], 8, 0, "click"]));
    assert_eq!(
        args(&saved["steps"], "/buffer_write")[1],
        json!({"s": "focus"}),
        "a path typed as the word is the word"
    );
}

/// **A file to open is the client's to open**, as a new one is: the form's
/// `OK` names the path in the outcome.
#[test]
fn the_open_file_form_names_a_path_for_the_client() {
    let (mut editor, ids) = with_dialogs(json!({"verb": "sync"}));
    let shown = event(&mut editor, json!([900, 1, 0, "open_file"]));
    assert!(
        shown["answer"]["corrections"]
            .to_string()
            .contains("\"index\":3")
    );
    let refused = event(&mut editor, json!([ids["open:ok"], 2, 0, "click"]));
    assert!(
        refused["answer"]["reason"]
            .as_str()
            .unwrap()
            .contains("select")
    );
    event(
        &mut editor,
        json!([
            ids["open:path"],
            3,
            0,
            "select",
            "/takes/b.flac",
            "/takes/c.wav"
        ]),
    );
    let opened = event(&mut editor, json!([ids["open:ok"], 4, 0, "click"]));
    assert_eq!(
        opened["openFile"],
        json!({"paths": ["/takes/b.flac", "/takes/c.wav"]})
    );
    assert!(opened.get("steps").is_none(), "nothing is written");
}
