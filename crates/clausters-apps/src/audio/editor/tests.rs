use super::*;

fn call(editor: &mut AudioEditor, request: Value) -> Value {
    serde_json::from_str(&call_json(editor, &request.to_string())).unwrap()
}

/// The steps an open operation carries out: the private copy and the join.
fn open(editor: &mut AudioEditor) -> Value {
    match editor.open_steps() {
        Ok(steps) => json!({ "steps": steps_json(&steps) }),
        Err(error) => json!({ "error": error }),
    }
}

/// The steps a save operation carries out, `request` as a save reads it
/// (`path` or `buffer`, and `format`), and where it writes.
fn save(editor: &mut AudioEditor, request: Value) -> Value {
    let format = request["format"].as_str().unwrap_or("float").to_string();
    let to = match (request["path"].as_str(), request["buffer"].as_i64()) {
        (Some(path), _) => Some(Target::File(path.to_string())),
        (None, Some(buffer)) => Some(Target::Buffer(buffer)),
        (None, None) => None,
    };
    match editor.save_steps(to, &format) {
        Ok(steps) => {
            let mut answer = json!({ "steps": steps_json(&steps) });
            match editor.target() {
                Target::File(path) => answer["path"] = json!(path),
                Target::Buffer(buffer) => answer["buffer"] = json!(buffer),
            }
            answer
        }
        Err(error) => json!({ "error": error }),
    }
}

/// A mono take of 100 frames in buffer 3 at 44.1 kHz, drawn through the join
/// in buffer 9, open in window 900 with its picture in widget 12, and buffers
/// 20 and 21 handed over for new takes.
fn opened(channels: u32) -> AudioEditor {
    let mut editor = new_json(
        &json!({"path":"/in/take3.wav","workspace":"/work", "frames": 100, "channels": channels, "rate": 44100,
                "display": 9})
        .to_string(),
    )
    .unwrap();
    call(&mut editor, json!({"verb": "window", "widget": 12}));
    call(&mut editor, json!({"verb": "sync", "window": 900}));
    editor
}

/// Each part as `(file, start, end)`: the stem of the file it reads.
fn spans(editor: &mut AudioEditor) -> Vec<(String, u64, u64)> {
    let parts = call(editor, json!({"verb": "parts"}))["parts"].clone();
    parts
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let r = &p["source"]["range"];
            let id = SourceId(p["source"]["source"].as_u64().unwrap());
            let path = editor.file.sources[&id].path.clone();
            let stem = std::path::Path::new(&path)
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            (
                stem,
                r["start"].as_u64().unwrap(),
                r["end"].as_u64().unwrap(),
            )
        })
        .collect()
}

fn span(file: &str, start: u64, end: u64) -> (String, u64, u64) {
    (file.to_string(), start, end)
}

/// The render steps of `steps`, in order.
fn renders(steps: &Value) -> Vec<Value> {
    steps
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s.get("render").cloned())
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
            } else if step.get("render").is_some() {
                "render".into()
            } else if step.get("move").is_some() {
                "move".into()
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

/// A message the window sends that asks for a save: whether it did, and the
/// steps the save operation it becomes would write, to where it now names.
fn window_save(editor: &mut AudioEditor, args: Value) -> (bool, Value) {
    let outcome = editor.event(
        &serde_json::from_value(json!({"addr": "/gui_event", "args": args})).unwrap(),
        0,
    );
    assert!(
        outcome.steps.is_none(),
        "a request writes nothing by itself"
    );
    let requested = outcome.save_requested;
    (requested, save(editor, json!({})))
}

fn event(editor: &mut AudioEditor, args: Value) -> Value {
    call(
        editor,
        json!({"verb": "event", "addr": "/gui_event", "version": 0, "args": args}),
    )
}

/// **Opening reads nothing of the file but its peaks, measured once**: a
/// render of the file into its peaks in the working folder, then the join
/// the server plays stitched over the file itself, which the window draws as
/// a list.
#[test]
fn opening_measures_the_file_and_stitches_the_join_over_it() {
    let mut editor = opened(1);
    let steps = open(&mut editor)["steps"].clone();
    assert_eq!(
        walk(&steps),
        ["render", "/buffer_stitch", "await /buffer_stitch"]
    );
    let measured = &renders(&steps)[0];
    assert_eq!(measured["path"], "", "the measure keeps no output");
    let stitched = args(&steps, "/buffer_stitch");
    assert_eq!(stitched[0], json!({"i": 9}), "the display buffer");
    assert_eq!(stitched[3], json!({"s": "/in/take3.wav"}), "reads the file");
    assert_eq!(spans(&mut editor), [span("take3", 0, 100)]);
    let def = call(&mut editor, json!({"verb": "window", "widget": 12}));
    let list = &def["children"][1]["segments"];
    assert_eq!(
        list["parts"][0]["path"], "/in/take3.wav",
        "the window draws the list"
    );
    assert!(
        list["parts"][0]["peaks"]
            .as_str()
            .unwrap()
            .starts_with("/work/peaks/take3-"),
        "the peaks are the working folder's"
    );
    assert!(def["children"][1].get("buffer").is_none());
}

/// **A file a part cannot read is opened through a buffer of it**: the
/// client decodes it, the open writes the buffer to the working folder, and
/// a save goes to the file -- whose format, a compressed one, a save then
/// refuses, so it is saved somewhere else.
#[test]
fn a_compressed_file_opens_through_its_buffer_and_saves_to_its_path() {
    let mut editor = new_json(
        &json!({"take": 3, "path": "/in/song.mp3", "frames": 100, "channels": 1,
                "rate": 44100, "display": 9, "workspace": "/work"})
        .to_string(),
    )
    .unwrap();
    call(&mut editor, json!({"verb": "window", "widget": 12}));
    let opened = open(&mut editor)["steps"].clone();
    assert_eq!(walk(&opened)[0], "/buffer_write");
    assert_eq!(spans(&mut editor), [span("9-opened", 0, 100)]);
    assert_eq!(
        call(&mut editor, json!({"verb": "file"}))["path"],
        "/in/song.mp3"
    );
}

/// **A take opened over a server buffer is a file of the working folder**,
/// written from the buffer by the open; a save renders the take and reads
/// the render back into the buffer, at the take's length.
#[test]
fn a_save_over_a_buffer_fills_it_from_a_render_of_the_take() {
    let mut editor = new_json(
        &json!({"take": 3, "frames": 100, "channels": 1, "rate": 44100, "display": 9,
                "workspace": "/work"})
        .to_string(),
    )
    .unwrap();
    call(&mut editor, json!({"verb": "window", "widget": 12}));
    let opened = open(&mut editor)["steps"].clone();
    assert_eq!(
        walk(&opened),
        [
            "/buffer_write",
            "await /buffer_write",
            "render",
            "/buffer_stitch",
            "await /buffer_stitch"
        ]
    );
    assert_eq!(
        args(&opened, "/buffer_write")[1],
        json!({"s": "/work/9-opened.w64"})
    );
    event(&mut editor, json!([12, 1, 0, "cut", 10.0, 20.0]));
    let saved = save(&mut editor, json!({}));
    assert_eq!(saved["buffer"], 3, "over the buffer it was opened from");
    let steps = &saved["steps"];
    assert_eq!(
        walk(steps)[..3],
        ["render", "/buffer_allocRead", "await /buffer_allocRead"]
    );
    assert_eq!(args(steps, "/buffer_allocRead")[0], json!({"i": 3}));
    let moved = save(&mut editor, json!({"buffer": 40}));
    assert_eq!(moved["buffer"], 40, "a save-as into another buffer");
    assert!(
        save(&mut editor, json!({"buffer": 9}))["error"].is_string(),
        "never the join"
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
        ["render", "/buffer_stitch", "await /buffer_stitch"]
    );
    let render = &renders(steps)[0];
    assert_eq!(
        render["path"], "/work/1-2-stroke.w64",
        "a file of the working folder"
    );
    assert_eq!(render["rate"], 44100.0, "at the take's rate");
    assert_eq!(
        spans(&mut editor),
        [
            span("take3", 0, 40),
            span("1-2-stroke", 0, 2),
            span("take3", 42, 100)
        ]
    );
    let leg = &out["record"]["legs"][0];
    assert_eq!(leg["forward"]["edit"]["intent"], "parts");
    assert_eq!(
        leg["backward"]["parts"].as_array().unwrap().len(),
        1,
        "the list before"
    );
    assert_eq!(leg["holdsForward"].as_array().unwrap().len(), 2);
    assert_eq!(leg["holdsBackward"].as_array().unwrap().len(), 1);
}

#[test]
fn a_stroke_over_a_wider_take_keeps_the_channels_it_was_not_drawn_on() {
    let mut editor = opened(2);
    let out = event(&mut editor, json!([12, 1, 0, "sample", 1, 10, 0.5, 0.0]));
    let render = &renders(&out["steps"])[0];
    assert_eq!(render["channels"], 2, "every channel of the take");
    assert_eq!(
        spans(&mut editor),
        [
            span("take3", 0, 10),
            span("1-2-stroke", 0, 1),
            span("take3", 11, 100)
        ]
    );
}

#[test]
fn a_stroke_with_no_working_folder_is_refused_and_redrawn() {
    let mut editor = opened(1);
    editor.window_state.workspace = None;
    let out = event(&mut editor, json!([12, 1, 0, "draw", 0, 0, [0.5], [0.0]]));
    assert!(out.get("steps").is_none());
    assert!(
        out["answer"]["reason"]
            .as_str()
            .unwrap()
            .contains("working folder")
    );
    assert_eq!(out["answer"]["corrections"][0]["widget"], 12);
    assert!(out["answer"]["corrections"][0]["props"]["segments"].is_object());
    assert_eq!(spans(&mut editor), [span("take3", 0, 100)]);
}

#[test]
fn a_cut_takes_the_span_out_and_writes_no_samples() {
    let mut editor = opened(1);
    let out = event(&mut editor, json!([12, 1, 0, "cut", 10.0, 20.0]));
    assert_eq!(
        walk(&out["steps"]),
        ["/buffer_stitch", "await /buffer_stitch"]
    );
    assert_eq!(
        spans(&mut editor),
        [span("take3", 0, 10), span("take3", 30, 100)]
    );
    assert_eq!(out["record"]["label"], "cut");
    assert_eq!(
        out["answer"]["corrections"],
        json!([
            {"widget": 12, "props": {"segments": editor.segments()}},
            {"widget": 12, "props": {"cursor": 10, "sel_start": 10, "sel_len": 0}}
        ]),
        "the picture is told the new list; and the selection is gone, the \
         cursor where the span was"
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

/// **A delete is a cut that copies nothing**: the span leaves the list, the
/// history calls it a delete, and the selection goes as a cut's does.
#[test]
fn a_delete_takes_the_span_out_as_a_cut_does() {
    let mut editor = opened(1);
    let out = event(&mut editor, json!([12, 1, 0, "delete", 10.0, 20.0]));
    assert_eq!(
        spans(&mut editor),
        [span("take3", 0, 10), span("take3", 30, 100)]
    );
    assert_eq!(out["record"]["label"], "delete");
    assert!(out.get("clipboard").is_none(), "nothing is copied");
    assert_eq!(out["selection"]["len"], 0.0);
    assert_eq!(out["cue"], 10);
}

#[test]
fn a_paste_of_a_block_is_a_file_of_its_own_put_in_at_the_position() {
    let mut editor = opened(1);
    let clip = Clipboard::samples(1, 3, 44100.0, 0);
    let doc = serde_json::to_string(&clip).unwrap();
    let out = event(
        &mut editor,
        json!([12, 1, 0, "paste", 50.0, "samples", doc, [0.1, 0.2, 0.3]]),
    );
    assert_eq!(
        walk(&out["steps"]),
        ["render", "/buffer_stitch", "await /buffer_stitch"]
    );
    assert_eq!(
        spans(&mut editor),
        [
            span("take3", 0, 50),
            span("1-2-pasted", 0, 3),
            span("take3", 50, 100)
        ]
    );
    assert_eq!(out["record"]["label"], "paste");
}

/// **A paste of segments puts their parts in**: nothing is made, the files
/// they name join the take's.
#[test]
fn a_paste_of_segments_names_their_files_and_makes_nothing() {
    let mut editor = opened(1);
    let part = SegmentPart {
        path: "/elsewhere/other.wav".into(),
        start: 1_000,
        frames: 20,
        fade_in: 0,
        fade_out: 0,
        fade_in_shape: Default::default(),
        fade_out_shape: Default::default(),
        channels: None,
        source_channels: 1,
        source_frames: 48_000,
        source_rate: 44_100.0,
    };
    let doc = Clipboard::segments(1, 44_100.0, vec![part]).to_json();
    let out = event(
        &mut editor,
        json!([12, 1, 0, "paste", 50.0, "segments", doc]),
    );
    assert_eq!(
        walk(&out["steps"]),
        ["/buffer_stitch", "await /buffer_stitch"]
    );
    assert_eq!(
        spans(&mut editor),
        [
            span("take3", 0, 50),
            span("other", 1_000, 1_020),
            span("take3", 50, 100)
        ]
    );
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
    assert_eq!(spans(&mut editor), [span("take3", 0, 100)]);
    // a mix is a sum taken sample for sample, so it is one rate
    let fast = serde_json::to_string(&Clipboard::samples(1, 1, 48000.0, 0)).unwrap();
    let out = event(
        &mut editor,
        json!([12, 2, 0, "mix", 0.0, "samples", fast, [0.1]]),
    );
    assert!(out["answer"]["reason"].as_str().unwrap().contains("Hz"));
    assert_eq!(spans(&mut editor), [span("take3", 0, 100)]);
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
    assert_eq!(
        renders(&out["steps"])[0]["rate"],
        48000.0,
        "the take keeps the block's rate"
    );
    assert_eq!(
        spans(&mut editor),
        [
            span("take3", 0, 50),
            span("1-2-pasted", 0, 480),
            span("take3", 50, 100)
        ]
    );
    // 480 frames at 48 kHz are 441 of the take at 44.1 kHz
    assert_eq!(call(&mut editor, json!({"verb": "parts"}))["frames"], 541);
}

#[test]
fn a_list_the_history_hands_back_is_the_take_again() {
    let mut editor = opened(1);
    let out = event(&mut editor, json!([12, 1, 0, "cut", 10.0, 20.0]));
    let before = out["record"]["legs"][0]["backward"].clone();
    let steps = editor.apply(&before).unwrap();
    assert_eq!(walk(&steps), ["/buffer_stitch", "await /buffer_stitch"]);
    assert_eq!(spans(&mut editor), [span("take3", 0, 100)]);
    assert!(editor.apply(&json!({"intent": "write"})).is_none());
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
        ["render", "/buffer_stitch", "await /buffer_stitch"]
    );
    // two frames: the block runs past the end of the take and is mixed as
    // far as it goes
    assert_eq!(
        spans(&mut editor),
        [span("take3", 0, 98), span("1-2-mixed", 0, 2)]
    );
    assert_eq!(out["record"]["label"], "mix");
}

/// **A save renders the take over its file**, and a file the list reads is
/// never written while it is read: the take is rendered beside it in the
/// working folder, the file moved there, the render moved where it was, and
/// the list names the moved file. A save-as names another file.
#[test]
fn a_save_renders_the_take_over_its_file_and_a_save_as_moves_it() {
    let mut editor = opened(1);
    event(&mut editor, json!([12, 1, 0, "cut", 10.0, 20.0]));
    let saved = save(&mut editor, json!({}));
    assert_eq!(saved["path"], "/in/take3.wav");
    assert_eq!(
        walk(&saved["steps"]),
        [
            "render",
            "move",
            "move",
            "/buffer_stitch",
            "await /buffer_stitch"
        ]
    );
    let moves: Vec<&Value> = saved["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s.get("move"))
        .collect();
    assert_eq!(
        moves[0]["from"], "/in/take3.wav",
        "the file the list reads, out of the way"
    );
    assert!(moves[0]["to"].as_str().unwrap().starts_with("/work/"));
    assert_eq!(moves[1]["to"], "/in/take3.wav", "the render where it was");
    assert!(
        editor
            .file
            .sources
            .values()
            .all(|s| s.path != "/in/take3.wav"),
        "the list reads the moved file"
    );
    let moved = save(
        &mut editor,
        json!({"path": "/takes/b.wav", "format": "int24"}),
    );
    assert_eq!(moved["path"], "/takes/b.wav");
    assert_eq!(
        walk(&moved["steps"])[0],
        "render",
        "a file nothing reads is rendered in place"
    );
    assert_eq!(renders(&moved["steps"])[0]["format"], "int24");
    assert_eq!(save(&mut editor, json!({}))["path"], "/takes/b.wav");
    assert!(save(&mut editor, json!({"format": "mp3"}))["error"].is_string());
}

#[test]
fn ctrl_s_over_the_window_is_a_save() {
    let mut editor = opened(1);
    call(&mut editor, json!({"verb": "sync", "path": "/takes/a.wav"}));
    let out = event(&mut editor, json!([900, 5, 0, "save"]));
    assert_eq!(out["answer"]["seq"], 5);
    assert!(out.get("record").is_none(), "a save is no edit");
    let (requested, saved) = window_save(&mut editor, json!([900, 6, 0, "save"]));
    assert!(requested);
    assert_eq!(walk(&saved["steps"])[0], "render");
    assert_eq!(saved["path"], "/takes/a.wav");
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
    call(&mut editor, json!({"verb": "looping", "on": true}));
    let play = call(&mut editor, json!({"verb": "play"}));
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
    call(&mut editor, json!({"verb": "looping", "on": false}));
    let whole = call(&mut editor, json!({"verb": "play"}));
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
/// move of the sweep says the span; its release marker says to put the head
/// into it without moving the cursor. A click empties the selection at its press
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
    let let_go = event(&mut editor, json!([12, 3, 0, "selection", 10, 20, true]));
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

/// A release names the exact selected frames even far into a take, while
/// preserving an independently placed position cursor.
#[test]
fn a_selection_far_into_a_take_is_matched_to_the_frame() {
    // An hour at 48 kHz, and an odd frame in it.
    let mut editor = new_json(
        &json!({"path":"/in/take3.wav","workspace":"/work", "frames": 200_000_000_u64, "channels": 1, "rate": 48000,
                "display": 9})
        .to_string(),
    )
    .unwrap();
    call(&mut editor, json!({"verb": "window", "widget": 12}));
    let far = 172_800_001_u64;
    event(&mut editor, json!([12, 1, 0, "selection", far, 20]));
    let beside = event(&mut editor, json!([12, 2, 0, "locate", far + 1]));
    assert!(beside.get("follow").is_none(), "a frame away is elsewhere");
    let let_go = event(&mut editor, json!([12, 3, 0, "selection", far, 20, true]));
    assert_eq!(let_go["follow"]["place"], json!(true));
    assert_eq!(editor.file.presentation.cursor, Some(far + 1));
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
    let ok = event(&mut editor.clone(), json!([ids["file:ok"], 3, 0, "click"]));
    assert!(
        ok["answer"]["corrections"]
            .to_string()
            .contains("\"index\":0")
    );
    let (requested, saved) = window_save(&mut editor, json!([ids["file:ok"], 3, 0, "click"]));
    assert!(requested);
    assert_eq!(renders(&saved["steps"])[0]["path"], "/takes/new.wav");
    let file = call(&mut editor, json!({"verb": "file"}));
    assert_eq!(file["path"], "/takes/new.wav");
    assert_eq!(file["untitled"], false);
    // and the next Ctrl+S writes there with no form
    let (requested, again) = window_save(&mut editor, json!([900, 4, 0, "save"]));
    assert!(requested);
    assert_eq!(
        walk(&again["steps"]),
        ["render", "/buffer_stitch", "await /buffer_stitch"]
    );
    assert_eq!(renders(&again["steps"])[0]["path"], "/takes/new.wav");
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
    let (_, saved) = window_save(&mut editor, json!([ids["file:ok"], 8, 0, "click"]));
    assert_eq!(
        renders(&saved["steps"])[0]["path"],
        "focus",
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

/// The props the answer `turned` corrects widget `id` with, merged.
fn corrections_of(turned: &Value, id: i64) -> Value {
    let mut props = serde_json::Map::new();
    for c in turned["answer"]["corrections"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if c["widget"] == id
            && let Some(map) = c["props"].as_object()
        {
            props.extend(map.clone());
        }
    }
    Value::Object(props)
}

/// The widget numbered `id` in a window's tree.
fn widget_in(tree: &Value, id: i64) -> Option<&Value> {
    if tree["id"] == id {
        return Some(tree);
    }
    tree["children"]
        .as_array()?
        .iter()
        .find_map(|child| widget_in(child, id))
}

/// **The open form is the shared forms mechanism over the host's chooser**:
/// it shows audio files, takes the keyboard, and -- cancelled and opened
/// again -- opens in the directory the hand went into, with nothing chosen,
/// so `OK` opens nothing chosen before.
#[test]
fn the_open_form_reopens_where_it_was_left_with_nothing_chosen() {
    let (mut editor, ids) = with_dialogs(json!({"verb": "sync"}));
    let chooser = ids["open:path"];
    let window = call(
        &mut editor,
        json!({"verb": "window", "widget": 12, "dialogs": ids}),
    );
    let drawn = widget_in(&window, chooser).expect("the chooser is in the window");
    assert_eq!(drawn["type"], "files");
    assert!(drawn["filter"].as_array().unwrap().contains(&json!("wav")));

    let shown = event(&mut editor, json!([900, 1, 0, "open_file"]));
    assert_eq!(
        corrections_of(&shown, chooser),
        json!({"path": ".", "focus": 1})
    );
    event(&mut editor, json!([chooser, 2, 0, "path", "/takes"]));
    event(
        &mut editor,
        json!([chooser, 3, 0, "select", "/takes/a.wav"]),
    );
    let cancelled = event(&mut editor, json!([ids["open:cancel"], 4, 0, "click"]));
    assert_eq!(
        corrections_of(&cancelled, ids["stack"]),
        json!({"index": 0})
    );
    assert!(cancelled.get("openFile").is_none());

    let again = event(&mut editor, json!([900, 5, 0, "open_file"]));
    assert_eq!(
        corrections_of(&again, chooser),
        json!({"path": "/takes", "focus": 1}),
        "where the hand left it"
    );
    let refused = event(&mut editor, json!([ids["open:ok"], 6, 0, "click"]));
    assert!(
        refused.get("openFile").is_none(),
        "nothing chosen this time"
    );
    assert!(
        refused["answer"]["reason"]
            .as_str()
            .unwrap()
            .contains("select")
    );
}

/// **A pick opens what is chosen and takes the form down**; a directory gone
/// into forgets the files chosen in the last one.
#[test]
fn a_pick_in_the_chooser_opens_the_files_it_names() {
    let (mut editor, ids) = with_dialogs(json!({"verb": "sync"}));
    let chooser = ids["open:path"];
    event(&mut editor, json!([900, 1, 0, "open_file"]));
    event(&mut editor, json!([chooser, 2, 0, "select", "/a/x.wav"]));
    event(&mut editor, json!([chooser, 3, 0, "path", "/b"]));
    let refused = event(&mut editor, json!([ids["open:ok"], 4, 0, "click"]));
    assert!(
        refused.get("openFile").is_none(),
        "chosen in a directory left behind"
    );
    let picked = event(
        &mut editor,
        json!([chooser, 5, 0, "pick", "/b/y.wav", "/b/z.flac"]),
    );
    assert_eq!(
        picked["openFile"],
        json!({"paths": ["/b/y.wav", "/b/z.flac"]})
    );
    assert_eq!(corrections_of(&picked, ids["stack"]), json!({"index": 0}));
    let after = event(&mut editor, json!([chooser, 6, 0, "pick", "/b/y.wav"]));
    assert!(after.get("openFile").is_none(), "no form is up to pick in");
}

/// **A window drawn again while the open form is up shows it again**, its
/// chooser where it was; a window closed has no form up when it opens again.
#[test]
fn a_redrawn_window_keeps_the_form_up_and_a_closed_one_drops_it() {
    let (mut editor, ids) = with_dialogs(json!({"verb": "sync"}));
    let chooser = ids["open:path"];
    event(&mut editor, json!([900, 1, 0, "open_file"]));
    event(&mut editor, json!([chooser, 2, 0, "path", "/takes"]));
    event(
        &mut editor,
        json!([chooser, 3, 0, "select", "/takes/a.wav"]),
    );
    let redrawn = call(
        &mut editor,
        json!({"verb": "window", "widget": 12, "dialogs": ids}),
    );
    assert_eq!(widget_in(&redrawn, ids["stack"]).unwrap()["index"], 3);
    assert_eq!(widget_in(&redrawn, chooser).unwrap()["path"], "/takes");
    let refused = event(&mut editor, json!([ids["open:ok"], 4, 0, "click"]));
    assert!(
        refused.get("openFile").is_none(),
        "the new chooser has nothing selected, so nothing is chosen"
    );

    call(
        &mut editor,
        json!({"verb": "event", "addr": "/gui_closed", "version": 0, "args": [900]}),
    );
    let reopened = call(
        &mut editor,
        json!({"verb": "window", "widget": 12, "dialogs": ids}),
    );
    assert_eq!(widget_in(&reopened, ids["stack"]).unwrap()["index"], 0);
    call(&mut editor, json!({"verb": "sync", "window": 900}));
    let ignored = event(&mut editor, json!([chooser, 5, 0, "pick", "/takes/a.wav"]));
    assert!(ignored.get("openFile").is_none());
}

/// **File > Close file names the file in focus by its identity**, which
/// `close_file` reads -- not by the buffer its join is drawn in. Closing the
/// last file leaves the editor open with none.
#[test]
fn the_close_file_entry_names_the_focused_file_by_its_identity() {
    let mut editor = opened(1);
    let asked = event(&mut editor, json!([900, 1, 0, "close_file"]));
    let action = &asked["fileAction"];
    assert_eq!(action["verb"], "close_file");
    let closed = call(
        &mut editor,
        json!({"verb": "close_file", "file": action["file"]}),
    );
    assert!(closed.get("error").is_none(), "{closed}");
    assert_eq!(closed["empty"], true);
    assert!(closed.get("close").is_none(), "the window is not closed");
    assert!(closed["binding"].is_null());
    assert_eq!(closed["restructure"], true, "drawn again, empty");
}

/// **An editor with no file open** is the application on its own: its window
/// keeps the menu bar, the file list and the dialogs and says how to open a
/// file where the take would be; the window's own verbs are answered and
/// every verb that acts on a file is refused, saying why.
#[test]
fn an_editor_opens_with_no_file_and_answers_only_the_window_s_verbs() {
    let mut editor = new_json(r#"{"empty":true,"display":9,"window":900}"#).unwrap();
    let names = call(&mut editor, json!({"verb": "dialogs"}))["dialogs"].clone();
    let ids: BTreeMap<String, i64> = names
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str().unwrap().to_string(), 400 + i as i64))
        .collect();
    let tree = call(
        &mut editor,
        json!({"verb": "window", "widget": 12, "filesWidget": 20, "dialogs": ids}),
    );
    let placeholder = widget_in(&tree, 12).expect("the take's place");
    assert_eq!(placeholder["type"], "label");
    assert_eq!(placeholder["text"], crate::samples::NO_FILE);
    assert!(tree["menu"].is_array(), "the menu bar stays");
    assert!(widget_in(&tree, 20).is_some(), "the file list stays");
    assert!(
        widget_in(&tree, ids["open:path"]).is_some(),
        "the dialogs stay"
    );
    assert!(!tree.to_string().contains("\"signal\""), "no take is drawn");

    assert_eq!(
        call(&mut editor, json!({"verb": "binding"})),
        json!({"empty": true})
    );
    assert_eq!(
        call(&mut editor, json!({"verb": "file"})),
        json!({"empty": true})
    );
    assert_eq!(
        call(&mut editor, json!({"verb": "playback"}))["active"],
        false
    );
    assert_eq!(
        call(&mut editor, json!({"verb": "files"}))["files"],
        json!([])
    );
    let playing = call(&mut editor, json!({"verb": "playing", "file": null}));
    assert!(
        playing["corrections"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["widget"] == 20),
        "nothing addressed to a take, pane or meter the window does not draw: {playing}"
    );
    for verb in ["parts", "save", "span", "play", "view_wave", "envelope"] {
        assert_eq!(
            call(&mut editor, json!({"verb": verb}))["error"],
            NO_FILE_OPEN,
            "{verb}"
        );
    }

    let saved = event(&mut editor, json!([900, 1, 0, "save"]));
    assert_eq!(saved["answer"]["reason"], NO_FILE_OPEN);
    let shown = event(&mut editor, json!([900, 2, 0, "open_file"]));
    assert_eq!(corrections_of(&shown, ids["stack"]), json!({"index": 3}));
    let picked = event(
        &mut editor,
        json!([ids["open:path"], 3, 0, "pick", "/a.wav"]),
    );
    assert_eq!(picked["openFile"], json!({"paths": ["/a.wav"]}));

    let actions = editor.actions(1, ServerInstance::default(), false, false, false);
    for verb in ["new_file", "open_file", "close", "keys", "toggle_files"] {
        assert!(actions[verb].enabled, "{verb}");
    }
    for verb in ["save", "cut", "close_file", "play"] {
        assert_eq!(
            actions[verb].reason.as_deref(),
            Some(NO_FILE_OPEN),
            "{verb}"
        );
    }
}

/// **The first file added to an empty editor** is the one in focus, and the
/// window draws it.
#[test]
fn a_file_added_to_an_empty_editor_comes_into_focus() {
    let mut editor = new_json(r#"{"empty":true,"display":9,"window":900}"#).unwrap();
    call(&mut editor, json!({"verb": "window", "widget": 12}));
    let added = call(
        &mut editor,
        json!({"verb": "add_file", "path":"/in/take50.wav","workspace":"/work", "frames": 200, "channels": 1,
               "rate": 48000, "display": 51, "path": "/a.wav"}),
    );
    assert!(added.get("error").is_none(), "{added}");
    assert_eq!(added["binding"]["display"], 51);
    let files = call(&mut editor, json!({"verb": "files"}))["files"].clone();
    assert_eq!(files.as_array().unwrap().len(), 1);
    let tree = call(&mut editor, json!({"verb": "window", "widget": 12}));
    assert_ne!(
        widget_in(&tree, 12).unwrap()["type"],
        "label",
        "the take is drawn"
    );
    assert_eq!(call(&mut editor, json!({"verb": "parts"}))["frames"], 200);
}

/// **A copy is the span's parts, by location**: no step, no buffer, a
/// clipboard of segments naming the files; a paste into another editor, at
/// another rate, puts the parts in and makes nothing. No sample moves.
#[test]
fn a_copy_names_the_span_s_files_and_a_paste_elsewhere_reads_them() {
    let mut editor = opened(1);
    let copied = event(&mut editor, json!([12, 1, 0, "copy", 10.0, 20.0]));
    assert!(copied.get("steps").is_none(), "nothing is made");
    assert!(copied.get("clipboardBuffer").is_none());
    let doc = copied["clipboard"].as_str().unwrap().to_string();
    let clip: Value = serde_json::from_str(&doc).unwrap();
    assert_eq!(clip["content"]["kind"], "segments");
    assert_eq!(clip["content"]["frames"], 20);
    assert_eq!(clip["content"]["parts"][0]["path"], "/in/take3.wav");
    assert_eq!(clip["content"]["parts"][0]["start"], 10);
    assert!(copied.get("record").is_none(), "a copy is not an edit");
    assert_eq!(spans(&mut editor), [span("take3", 0, 100)]);

    let mut other = new_json(
        &json!({"path":"/in/take30.wav","workspace":"/work2", "frames": 100, "channels": 1, "rate": 48000,
                "display": 31})
        .to_string(),
    )
    .unwrap();
    call(&mut other, json!({"verb": "window", "widget": 12}));
    let pasted = event(
        &mut other,
        json!([12, 1, 0, "paste", 50.0, "segments", doc]),
    );
    assert_eq!(
        walk(&pasted["steps"]),
        ["/buffer_stitch", "await /buffer_stitch"]
    );
    assert_eq!(
        spans(&mut other),
        [
            span("take30", 0, 50),
            span("take3", 10, 30),
            span("take30", 50, 100)
        ]
    );
    // 20 frames at 44.1 kHz are 22 of a take at 48 kHz, read at the ratio
    assert_eq!(call(&mut other, json!({"verb": "parts"}))["frames"], 122);
}

#[test]
fn a_cut_waits_for_copy_completion_and_checks_its_original_target() {
    let mut editor = opened(1);
    let prepared = event(&mut editor, json!([12, 1, 0, "copy_cut", 10, 20]));
    assert!(prepared["clipboardCut"].is_object());
    assert!(!prepared["changed"].as_bool().unwrap_or(false));
    assert_eq!(spans(&mut editor), [span("take3", 0, 100)]);
    let completed = editor.act(&prepared["clipboardCut"], 0);
    assert!(completed.changed);
    assert_eq!(
        spans(&mut editor),
        [span("take3", 0, 10), span("take3", 30, 100)]
    );
    let duplicate = editor.act(&prepared["clipboardCut"], 1);
    assert!(!duplicate.changed);
    assert_eq!(
        spans(&mut editor),
        [span("take3", 0, 10), span("take3", 30, 100)]
    );
}

#[test]
fn a_copy_mixes_into_a_span_and_a_width_or_rate_mismatch_refuses() {
    let mut editor = opened(1);
    let copied = event(&mut editor, json!([12, 1, 0, "copy", 0, 20]));
    let doc = copied["clipboard"].as_str().unwrap();
    let mixed = event(&mut editor, json!([12, 2, 0, "mix", 90, "segments", doc]));
    assert!(mixed["changed"].as_bool().unwrap());
    assert_eq!(
        walk(&mixed["steps"]),
        ["render", "/buffer_stitch", "await /buffer_stitch"]
    );
    assert_eq!(
        spans(&mut editor),
        [span("take3", 0, 90), span("1-2-mixed", 0, 10)]
    );
    let mut invalid: Value = serde_json::from_str(doc).unwrap();
    invalid["content"]["sample_rate"] = json!(48000);
    let refused = event(
        &mut editor,
        json!([12, 3, 1, "mix", 0, "segments", invalid.to_string()]),
    );
    assert!(refused["answer"]["reason"].is_string());
    assert!(refused.get("steps").is_none());
    invalid["content"]["channels"] = json!(2);
    let refused = event(
        &mut editor,
        json!([12, 4, 1, "paste", 0, "segments", invalid.to_string()]),
    );
    assert!(refused["answer"]["reason"].is_string());
    assert!(refused.get("steps").is_none());
}

#[test]
fn truncated_and_non_numeric_sample_payloads_refuse_without_allocating() {
    for samples in [
        json!([0.1]),
        json!([0.1, 0.2, 0.3, 0.4]),
        json!([0.1, "bad", 0.3]),
    ] {
        let mut editor = opened(1);
        let doc = Clipboard::samples(1, 3, 44100.0, 0).to_json();
        let refused = event(
            &mut editor,
            json!([12, 1, 0, "paste", 0, "samples", doc, samples]),
        );
        assert!(refused["answer"]["reason"].is_string());
        assert!(refused.get("steps").is_none());
        assert_eq!(spans(&mut editor), [span("take3", 0, 100)]);
    }
}

/// **A window coming back looks at its files on disk**: the activation asks
/// for the opened file's stamp, a stamp that moved is said on the status
/// bar once, and a file gone is said too.
#[test]
fn a_file_changed_on_disk_is_said_when_the_window_comes_back() {
    let stamp = |modified: f64| json!({"modified": modified, "size": 4_000});
    let mut editor = new_json(
        &json!({"path": "/in/take3.wav", "workspace": "/work", "frames": 100, "channels": 1,
                "rate": 44100, "display": 9, "stamp": stamp(1.0)})
        .to_string(),
    )
    .unwrap();
    call(&mut editor, json!({"verb": "window", "widget": 12}));
    call(&mut editor, json!({"verb": "sync", "window": 900}));
    let back = event(&mut editor, json!([900, 4, 0, "activated", 1]));
    assert_eq!(back["checkDisk"]["paths"], json!(["/in/take3.wav"]));
    assert_eq!(back["checkDisk"]["seq"], 4);
    assert!(
        back["answer"].is_null(),
        "answered once the files are looked at"
    );
    let away = event(&mut editor, json!([900, 5, 0, "activated", 0]));
    assert!(
        away.get("checkDisk").is_none(),
        "a window put away asks for nothing"
    );

    let look = |editor: &mut AudioEditor, found: Value| {
        call(
            editor,
            json!({"verb": "on_disk", "seq": 4,
            "stamps": {"/in/take3.wav": found}}),
        )["answer"]
            .clone()
    };
    assert!(
        look(&mut editor, stamp(1.0))["reason"].is_null(),
        "unchanged"
    );
    assert_eq!(
        look(&mut editor, stamp(2.0))["reason"],
        "take3.wav changed on disk"
    );
    assert!(
        look(&mut editor, stamp(2.0))["reason"].is_null(),
        "said once"
    );
    assert_eq!(
        look(&mut editor, Value::Null)["reason"],
        "take3.wav is no longer on disk"
    );
    assert!(
        event(&mut editor, json!([900, 6, 0, "activated", 1]))
            .get("checkDisk")
            .is_none(),
        "a file gone is looked for no more"
    );
}
