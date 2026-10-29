//! **The audio editor's playback, carried out on an engine the application
//! holds**: the same playback a client walks as OSC steps answers no steps
//! here, its ops built into the engine's commands at once, and the take is
//! heard frame for frame.

#![cfg(feature = "synth")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use clausters::dsp::buffer::Buffer;
use clausters::rosc::OscType;
use clausters::server::engine::{BLOCK_SIZE, Cmd, engine_pair};
use clausters::server::nrt::{NrtAction, run_job};
use clausters_app_engine::{AppEngine, EngineMedium};
use clausters_core::ids::{IdShare, IdSpaces, ServerShape, Space};
use clausters_editing::audio_playback::AudioEditorPlayback;
use clausters_editing::pass::Pass;

const SR: f64 = 48_000.0;
const FRAMES: usize = 24_000;

/// **Where the transport rolled**, and where the play's 5 ms ramp is over.
/// A play waits for its cue, so the transport rolls once its readers hold
/// the frame it starts on: the first block that sounds is the start of the
/// pass, and from the end of the ramp on the output is the take. The cue
/// is over when the reader's thread has filled, which a loaded machine
/// delays.
fn rolled(heard: &[f32]) -> (usize, usize) {
    let first = heard
        .iter()
        .position(|&s| s != 0.0)
        .expect("the take was heard");
    assert!(first < 12_000, "the cue took {first} samples");
    let rolled = first / BLOCK_SIZE * BLOCK_SIZE;
    (rolled, rolled + 256)
}

/// A mono float file whose every frame is told apart from every other.
fn take(name: &str) -> (String, Vec<f32>) {
    let values: Vec<f32> = (0..FRAMES)
        .map(|n| 0.25 + n as f32 / (1 << 20) as f32)
        .collect();
    let path = std::env::temp_dir().join(format!("app-engine-{name}-{}.wav", std::process::id()));
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SR as u32,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for &v in &values {
        w.write_sample(v).unwrap();
    }
    w.finalize().unwrap();
    (path.to_string_lossy().into_owned(), values)
}

/// The join of one part over the whole file.
fn join(path: &str) -> Arc<Buffer> {
    let args = vec![
        OscType::Int(9),
        OscType::Int(1),
        OscType::Float(SR as f32),
        OscType::String(path.into()),
        OscType::Int(0),
        OscType::Int(FRAMES as i32),
        OscType::Int(0),
        OscType::Int(0),
        OscType::Int(1),
        OscType::Float(0.0),
        OscType::Int(1),
        OscType::Float(0.0),
        OscType::Int(0),
    ];
    let mirror = vec![None; 16];
    let (_, job) =
        clausters::osc::translate::parse_buffer_msg("/buffer_stitch", &args, &mirror, SR)
            .expect("parsed");
    match run_job(job).expect("stitched") {
        NrtAction::Install(b) => b,
        _ => panic!("a stitch installs a buffer"),
    }
}

/// **The take is heard as it is, with nothing sent but commands**: every
/// call of the playback answers no steps, and once the reader's first refill
/// and the play's ramp are behind it, both sides play the file's frames.
#[test]
fn the_audio_editor_plays_on_an_engine_it_holds() {
    let (path, values) = take("plays");
    let (mut engine, handle) = engine_pair(SR as f32, 2);
    let app = Arc::new(Mutex::new(AppEngine::over(handle)));
    let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
    let bufnum = ids.alloc(Space::Buffers, 1).unwrap() as i32;
    assert!(app.lock().unwrap().push(vec![Cmd::SetBuffer {
        index: bufnum as usize,
        buffer: Some(join(&path)),
    }]));
    let mut playback = AudioEditorPlayback::in_medium(EngineMedium::new(Arc::clone(&app)));
    let steps = playback
        .sync(1, bufnum, 1, FRAMES as u64, SR, SR, &mut ids)
        .expect("synced");
    assert!(steps.is_empty(), "nothing to walk: {steps:?}");
    let steps = playback.play(
        1,
        0,
        Pass::Until {
            end: Some(FRAMES as u64),
            back: 0,
        },
    );
    assert!(steps.is_empty(), "nothing to walk: {steps:?}");
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for _ in 0..(FRAMES / BLOCK_SIZE) {
        engine.process_block(&mut out);
        app.lock().unwrap().pump();
        for f in out.as_chunks::<2>().0 {
            left.push(f[0]);
            right.push(f[1]);
        }
        std::thread::sleep(Duration::from_micros(1_333));
    }
    // From where it rolled, past the play's ramp, the output is the file.
    let (at, from) = rolled(&left);
    for i in from..16_000 {
        assert_eq!(left[i], values[i - at], "left, sample {i}");
        assert_eq!(right[i], values[i - at], "right, sample {i}");
    }
    let _ = std::fs::remove_file(&path);
}

/// Blocks of the engine, paced, with the application's turn after each:
/// the left side.
fn blocks(
    engine: &mut clausters::server::engine::Engine,
    app: &Mutex<AppEngine>,
    n: usize,
) -> Vec<f32> {
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    let mut left = Vec::new();
    for _ in 0..n {
        engine.process_block(&mut out);
        app.lock().unwrap().pump();
        left.extend(out.as_chunks::<2>().0.iter().map(|f| f[0]));
        std::thread::sleep(Duration::from_micros(1_333));
    }
    left
}

/// **A loop, a stop and a locate are the engine's commands too**: a pass
/// over a loop wraps on its span, a stop falls silent, and a play from a
/// located frame plays from there.
#[test]
fn loops_stops_and_locates_on_an_engine_it_holds() {
    let (path, values) = take("loops");
    let (mut engine, handle) = engine_pair(SR as f32, 2);
    let app = Arc::new(Mutex::new(AppEngine::over(handle)));
    let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
    let bufnum = ids.alloc(Space::Buffers, 1).unwrap() as i32;
    app.lock().unwrap().push(vec![Cmd::SetBuffer {
        index: bufnum as usize,
        buffer: Some(join(&path)),
    }]);
    let mut playback = AudioEditorPlayback::in_medium(EngineMedium::new(Arc::clone(&app)));
    playback
        .sync(1, bufnum, 1, FRAMES as u64, SR, SR, &mut ids)
        .unwrap();
    let (from, to) = (2_000u64, 6_000u64);
    playback.play(1, from, Pass::Loop { from, to });
    let heard = blocks(&mut engine, &app, 300);
    // Past the ramp, every sample is the loop's frame.
    let span = (to - from) as usize;
    let (at, settled) = rolled(&heard);
    for (i, &s) in heard.iter().enumerate().skip(settled) {
        let frame = from as usize + (i - at) % span;
        assert_eq!(s, values[frame], "sample {i}, frame {frame}");
    }
    // A stop rolls out its ramp and is silent after it.
    playback.stop(0);
    let after = blocks(&mut engine, &app, 20);
    assert!(
        after[after.len() - BLOCK_SIZE..].iter().all(|&s| s == 0.0),
        "silent once stopped"
    );
    // A play from a located frame plays from there.
    let at = 15_000u64;
    playback.play(
        1,
        at,
        Pass::Until {
            end: Some(FRAMES as u64),
            back: 0,
        },
    );
    let heard = blocks(&mut engine, &app, 200);
    let (start, settled) = rolled(&heard);
    for (i, &s) in heard.iter().enumerate().skip(settled) {
        let frame = at as usize + i - start;
        // The stop's ramp ends on the end mark, 5 ms before it.
        if frame >= FRAMES - 512 {
            break;
        }
        assert_eq!(s, values[frame], "sample {i} after the locate");
    }
    let _ = std::fs::remove_file(&path);
}

/// A stereo float file whose left and right are told apart, as are its
/// frames.
fn stereo(name: &str) -> (String, Vec<[f32; 2]>) {
    let values: Vec<[f32; 2]> = (0..FRAMES)
        .map(|n| {
            let f = n as f32 / (1 << 20) as f32;
            [0.25 + f, -0.25 - f]
        })
        .collect();
    let path = std::env::temp_dir().join(format!("app-engine-{name}-{}.wav", std::process::id()));
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SR as u32,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for frame in &values {
        w.write_sample(frame[0]).unwrap();
        w.write_sample(frame[1]).unwrap();
    }
    w.finalize().unwrap();
    (path.to_string_lossy().into_owned(), values)
}

/// One part of the stereo file.
fn part_of(path: &str, start: u64, frames: u64) -> clausters_document::clipboard::SegmentPart {
    clausters_document::clipboard::SegmentPart {
        path: path.into(),
        start,
        frames,
        fade_in: 0,
        fade_out: 0,
        fade_in_shape: Default::default(),
        fade_out_shape: Default::default(),
        channels: None,
        source_channels: 2,
        source_frames: FRAMES as u64,
        source_rate: SR,
    }
}

/// Both sides, as `blocks` gives the left.
fn both(
    engine: &mut clausters::server::engine::Engine,
    app: &Mutex<AppEngine>,
    n: usize,
) -> (Vec<f32>, Vec<f32>) {
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for _ in 0..n {
        engine.process_block(&mut out);
        app.lock().unwrap().pump();
        for f in out.as_chunks::<2>().0 {
            left.push(f[0]);
            right.push(f[1]);
        }
        std::thread::sleep(Duration::from_micros(1_333));
    }
    (left, right)
}

/// **The application reads its take, and the engine plays what it is
/// handed**: a stereo take of two parts over one file, read by the
/// application's own thread into a mono fed stream per channel, plays each
/// side's frames; an edit (the parts put the other way round) is a new list
/// the readers fade into from where they are, and then plays as edited. No
/// list, no stitch and no file is in the engine.
#[test]
fn the_application_reads_its_take_into_the_engine() {
    use clausters_app_engine::take::{Headers, ReadList, TakeFeed};

    let (path, values) = stereo("take");
    let (mut engine, handle) = engine_pair(SR as f32, 2);
    let app = Arc::new(Mutex::new(AppEngine::over(handle)));
    let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
    let mut headers = Headers::default();
    let half = (FRAMES / 2) as u64;
    let first = vec![part_of(&path, 0, half), part_of(&path, half, half)];
    let list = ReadList::of(&first, 2, SR, &mut headers).expect("the file reads");
    let feed = TakeFeed::start(2);
    feed.set(Arc::new(list));
    let buffers = AppEngine::alloc_take(2, &mut ids).unwrap();
    assert!(
        app.lock()
            .unwrap()
            .install_take(&feed, FRAMES as u64, &buffers)
    );
    let mut playback = AudioEditorPlayback::in_medium(EngineMedium::new(Arc::clone(&app)));
    playback
        .sync_streams(1, buffers.clone(), FRAMES as u64, SR, SR, &mut ids)
        .unwrap();
    playback.play(
        1,
        0,
        Pass::Until {
            end: Some(FRAMES as u64),
            back: 0,
        },
    );
    let (left, right) = both(&mut engine, &app, 120);
    let (start, from) = rolled(&left);
    for i in from..left.len() {
        assert_eq!(left[i], values[i - start][0], "left, sample {i}");
        assert_eq!(right[i], values[i - start][1], "right, sample {i}");
    }
    // The edit: the second half first.
    let edited = vec![part_of(&path, half, half), part_of(&path, 0, half)];
    feed.set(Arc::new(
        ReadList::of(&edited, 2, SR, &mut headers).unwrap(),
    ));
    let played = left.len() - start;
    let (left, right) = both(&mut engine, &app, 60);
    let mut zeros = 0;
    for (i, &s) in left.iter().enumerate() {
        zeros = if s == 0.0 { zeros + 1 } else { 0 };
        assert!(
            zeros <= 2,
            "sample {i}: a silence where the take was edited"
        );
    }
    let frame = |i: usize| {
        let at = played + i;
        if at < half as usize {
            at + half as usize
        } else {
            at - half as usize
        }
    };
    for i in 2_000..left.len() {
        assert_eq!(
            left[i],
            values[frame(i)][0],
            "left after the edit, sample {i}"
        );
        assert_eq!(
            right[i],
            values[frame(i)][1],
            "right after the edit, sample {i}"
        );
    }
    let _ = std::fs::remove_file(&path);
}

/// **An action of the audio editor carried to its end** by the application
/// that holds the engine: a render it handed off is waited for, the engine
/// processed meanwhile as its device would.
fn act(
    app: &mut clausters_app_engine::app::App,
    editing: &mut clausters_apps::editing::Editing,
    engine: &mut clausters::server::engine::Engine,
    member: u32,
    action: serde_json::Value,
) -> clausters_app_engine::app::Ran {
    let mut ran = app.act(editing, member, action, 7, None);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    let mut effects = std::mem::take(&mut ran.effects);
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while ran.end.is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "the render never landed"
        );
        engine.process_block(&mut out);
        std::thread::sleep(Duration::from_millis(1));
        for mut landed in app.landed(editing) {
            effects.append(&mut landed.effects);
            if landed.end.is_some() {
                assert_eq!(landed.token, 7, "the end comes back with its token");
                ran = landed;
            }
        }
    }
    ran.effects = effects;
    let end = ran.end.as_ref().unwrap();
    assert!(end.is_ok(), "{end:?}");
    ran
}

/// The editing context's version, which an event is made against.
fn version(
    app: &mut clausters_app_engine::app::App,
    editing: &mut clausters_apps::editing::Editing,
) -> serde_json::Value {
    app.call(editing, &serde_json::json!({"verb": "state"}))["version"].clone()
}

/// **The audio editor, played on an engine its application holds**: its
/// coordinator opens a file, measures its peaks, plays it from its first
/// frame, and after a delete and a reverse plays the take as edited -- the
/// list read by the application into the engine's fed streams, the renders
/// run in the application's threads, nothing stitched and no OSC between
/// them.
#[test]
fn the_audio_editor_s_coordinator_plays_its_file_on_an_engine_it_holds() {
    use clausters_app_engine::app::App;
    use clausters_apps::editing::Editing;
    use serde_json::json;

    let (path, values) = take("coordinator");
    let workspace = std::env::temp_dir().join(format!("app-engine-ws-{}", std::process::id()));
    std::fs::create_dir_all(&workspace).unwrap();
    let (mut engine, handle) = engine_pair(SR as f32, 2);
    let mut app = App::new(Arc::new(Mutex::new(AppEngine::over(handle))));
    let mut editing = Editing::new();
    app.hold(&mut editing);
    let opened = app.call(
        &mut editing,
        &json!({"verb": "openAudio", "key": "take", "path": path,
            "workspace": workspace.to_string_lossy(), "frames": FRAMES,
            "channels": 1, "rate": SR}),
    );
    let member = opened["member"].as_u64().expect("an audio member") as u32;
    for call in [
        json!({"verb": "sync", "window": 900}),
        json!({"verb": "window", "widget": 12}),
    ] {
        app.call(
            &mut editing,
            &json!({"verb": "member", "member": member, "call": call}),
        );
    }
    let opened = act(
        &mut app,
        &mut editing,
        &mut engine,
        member,
        json!({"verb": "open", "empty": false}),
    );
    let result = opened.end.unwrap().unwrap();
    assert!(result["transport"].is_i64(), "{result}");
    // the file opened with no peaks has them measured, by the application
    let measured = std::fs::read_dir(&workspace)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "peaks"))
        .filter_map(|e| e.metadata().ok())
        .any(|m| m.len() > 0);
    assert!(measured, "the peaks were measured into the working folder");
    let app_engine = Arc::clone(app.engine());
    act(
        &mut app,
        &mut editing,
        &mut engine,
        member,
        json!({"verb": "play", "at": 0.0}),
    );
    let heard = blocks(&mut engine, &app_engine, 150);
    let (start, from) = rolled(&heard);
    for (i, &s) in heard.iter().enumerate().skip(from) {
        assert_eq!(s, values[i - start], "sample {i}");
    }
    // what it plays, as a script asks: its meter reads the take, and no
    // frame was missing
    let output = app.query(&mut editing, member, "output");
    assert!(output["levels"][0].as_f64().unwrap() > 0.2, "{output}");
    assert_eq!(output["starved"], 0, "{output}");
    // A cursor placed while it plays moves the music there.
    let v = version(&mut app, &mut editing);
    act(
        &mut app,
        &mut editing,
        &mut engine,
        member,
        json!({"verb": "event", "addr": "/gui_event", "window": true,
            "args": [12, 9, v, "locate", 18_000.0]}),
    );
    let reading = app_engine.lock().unwrap().reading();
    let transport = app.query(&mut editing, member, "transport")["transport"]
        .as_u64()
        .unwrap() as usize;
    blocks(&mut engine, &app_engine, 1);
    let at = reading.transport_position(transport) as usize;
    assert!(
        (18_000..18_000 + 2 * BLOCK_SIZE).contains(&at),
        "located while it plays: {at}"
    );
    // The first half deleted: the take is the second half from now on.
    act(
        &mut app,
        &mut editing,
        &mut engine,
        member,
        json!({"verb": "stop"}),
    );
    blocks(&mut engine, &app_engine, 20);
    let half = FRAMES / 2;
    let v = version(&mut app, &mut editing);
    act(
        &mut app,
        &mut editing,
        &mut engine,
        member,
        json!({"verb": "event", "addr": "/gui_event",
            "args": [12, 1, v, "delete", 0.0, half as f64]}),
    );
    act(
        &mut app,
        &mut editing,
        &mut engine,
        member,
        json!({"verb": "play", "at": 0.0}),
    );
    let heard = blocks(&mut engine, &app_engine, 150);
    let (start, from) = rolled(&heard);
    for (i, &s) in heard.iter().enumerate().skip(from) {
        let frame = i - start;
        if frame >= half - 512 {
            break;
        }
        assert_eq!(s, values[half + frame], "sample {i} of the edited take");
    }
    // A reverse of the first 4000 frames is a render the list then names.
    act(
        &mut app,
        &mut editing,
        &mut engine,
        member,
        json!({"verb": "stop"}),
    );
    blocks(&mut engine, &app_engine, 20);
    let v = version(&mut app, &mut editing);
    let reversed = act(
        &mut app,
        &mut editing,
        &mut engine,
        member,
        json!({"verb": "event", "addr": "/gui_event", "window": true,
            "args": [12, 2, v, "reverse", 0.0, 4000.0]}),
    );
    assert!(
        reversed.effects.iter().any(|e| e["kind"] == "progress"),
        "the render's progress is said: {:?}",
        reversed.effects
    );
    act(
        &mut app,
        &mut editing,
        &mut engine,
        member,
        json!({"verb": "play", "at": 0.0}),
    );
    let heard = blocks(&mut engine, &app_engine, 150);
    let (start, from) = rolled(&heard);
    for (i, &s) in heard.iter().enumerate().skip(from) {
        let frame = i - start;
        if frame >= 4000 {
            break;
        }
        assert_eq!(
            s,
            values[half + 3999 - frame],
            "sample {i} of the reversed span"
        );
    }
    // An undo puts the delete's list back with nothing to carry out, and
    // the take plays from its first frame again.
    act(
        &mut app,
        &mut editing,
        &mut engine,
        member,
        json!({"verb": "stop"}),
    );
    blocks(&mut engine, &app_engine, 20);
    for seq in [3, 4] {
        let v = version(&mut app, &mut editing);
        act(
            &mut app,
            &mut editing,
            &mut engine,
            member,
            json!({"verb": "event", "addr": "/gui_event", "window": true,
                "args": [900, seq, v, "undo"]}),
        );
    }
    act(
        &mut app,
        &mut editing,
        &mut engine,
        member,
        json!({"verb": "play", "at": 0.0}),
    );
    let heard = blocks(&mut engine, &app_engine, 150);
    let (start, from) = rolled(&heard);
    for (i, &s) in heard.iter().enumerate().skip(from) {
        assert_eq!(s, values[i - start], "sample {i} after the undos");
    }
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir_all(&workspace);
}

/// **A run of `run` the reader did not starve**, its result: one in which
/// the engine counted silence for want of a frame says nothing about the
/// change the test turns, since the machine, not the change, made it. Run
/// again, up to four times; a machine that starves every run fails the test
/// saying so, rather than passing it.
fn unstarved<T>(fed: bool, mut run: impl FnMut() -> (T, u64)) -> T {
    let mut starved = 0;
    for _ in 0..4 {
        let (result, short) = run();
        if short == 0 {
            return result;
        }
        starved = short;
    }
    panic!("fed {fed}: the reader ran out of lead in every run ({starved} samples the last)");
}

/// What a moved loop's run heard: the samples where the output stepped, and
/// the blocks where the position jumped (`(block, from, to)`).
type Moved = (Vec<usize>, Vec<(usize, u64, u64)>);

/// **A loop moved while it plays, as a selection dragged over a looping
/// take moves it, and the loop switch turned off**: each change is a
/// reading of the take replaced while it plays, which is crossfaded, so the
/// output never steps -- but where the loop itself comes round. Over the
/// application's fed stream (`fed`) and over the reader's own thread alike.
fn moved_loop_steps(fed: bool) -> (Moved, u64) {
    use clausters_app_engine::take::{Headers, ReadList, TakeFeed};
    let (path, _) = take(if fed { "moved-fed" } else { "moved-own" });
    let (mut engine, handle) = engine_pair(SR as f32, 2);
    let app = Arc::new(Mutex::new(AppEngine::over(handle)));
    let reading = app.lock().unwrap().reading();
    let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
    let mut playback = AudioEditorPlayback::in_medium(EngineMedium::new(Arc::clone(&app)));
    let feed = TakeFeed::start(1);
    if fed {
        let part = clausters_document::clipboard::SegmentPart {
            path: path.clone(),
            start: 0,
            frames: FRAMES as u64,
            fade_in: 0,
            fade_out: 0,
            fade_in_shape: Default::default(),
            fade_out_shape: Default::default(),
            channels: None,
            source_channels: 1,
            source_frames: FRAMES as u64,
            source_rate: SR,
        };
        let list = ReadList::of(&[part], 1, SR, &mut Headers::default()).unwrap();
        feed.set(Arc::new(list));
        let buffers = AppEngine::alloc_take(1, &mut ids).unwrap();
        app.lock()
            .unwrap()
            .install_take(&feed, FRAMES as u64, &buffers);
        playback
            .sync_streams(1, buffers, FRAMES as u64, SR, SR, &mut ids)
            .unwrap();
    } else {
        let bufnum = ids.alloc(Space::Buffers, 1).unwrap() as i32;
        app.lock().unwrap().push(vec![Cmd::SetBuffer {
            index: bufnum as usize,
            buffer: Some(join(&path)),
        }]);
        playback
            .sync(1, bufnum, 1, FRAMES as u64, SR, SR, &mut ids)
            .unwrap();
    }
    let transport = playback.transport().unwrap_or(0) as usize;
    playback.play(
        1,
        2000,
        Pass::Loop {
            from: 2000,
            to: 14000,
        },
    );
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    let (mut heard, mut at) = (Vec::new(), Vec::new());
    for b in 0..450 {
        // the drag: the loop's end moves every four blocks
        if (100..300).contains(&b) && b % 4 == 0 {
            playback.follow(1, 2000, 14_000 + (b - 100) as u64 * 20, false);
        }
        if b == 400 {
            playback.set_pass(Pass::Until {
                end: Some(FRAMES as u64),
                back: 2000,
            });
        }
        engine.process_block(&mut out);
        app.lock().unwrap().pump();
        heard.extend(out.as_chunks::<2>().0.iter().map(|f| f[0]));
        at.push(reading.transport_position(transport));
        std::thread::sleep(Duration::from_micros(1_333));
    }
    // The take is a ramp that rises 2^-20 a frame: past the play's 5 ms
    // ramp, a step is a discontinuity.
    let steps = (600..heard.len())
        .filter(|&i| (heard[i] - heard[i - 1]).abs() > 0.005)
        .collect();
    // a transport held by its cue stands still, which is not a jump
    let jumps = (1..at.len())
        .filter(|&b| at[b] != at[b - 1] && at[b] != at[b - 1] + BLOCK_SIZE as u64)
        .map(|b| (b, at[b - 1], at[b]))
        .collect();
    let _ = std::fs::remove_file(&path);
    let starved = app.lock().unwrap().handle().meters().starved();
    ((steps, jumps), starved)
}

#[test]
fn a_loop_moved_while_it_plays_is_crossfaded() {
    for fed in [true, false] {
        let (steps, jumps) = unstarved(fed, || moved_loop_steps(fed));
        // the position never moves but where the loop comes round, once
        assert_eq!(jumps.len(), 1, "fed {fed}: {jumps:?}");
        let wrap = jumps[0].0;
        assert!(
            jumps[0].2 < jumps[0].1,
            "fed {fed}: the loop came round: {jumps:?}"
        );
        // and the output steps only there: in the block the position
        // jumped in, the one before, or -- a wrap on a block's last frame --
        // at the first frame of the next
        assert!(
            steps.iter().all(|&i| i / BLOCK_SIZE + 1 == wrap
                || i / BLOCK_SIZE == wrap
                || i == (wrap + 1) * BLOCK_SIZE),
            "fed {fed}: steps at {steps:?}, the loop came round in block {wrap}"
        );
    }
}

/// **The loop switch turned while it plays changes nothing that is heard**:
/// each turn is a reading replaced while it plays, and a reading that
/// agrees with the one before it is crossfaded into itself. The engine runs
/// in bursts of sixteen blocks, as a device's callback runs it, and the
/// switch turns at a hand's pace, and once a pass just before the loop's
/// end, apart from the turn before it (two turns that close are the next
/// test's); every block the transport plays straight through is the take's
/// frames at its position, sample for sample. A loop set, moved or cleared is the same reading up to where the
/// two part, and the new one is spliced on there; the change is cued, so one
/// turned too near the loop's end for the new reading to be read in time
/// takes effect a pass later. Over the application's fed stream and over the
/// reader's own thread alike.
#[test]
fn the_loop_switch_turned_while_it_plays_is_not_heard() {
    use clausters_app_engine::take::{Headers, ReadList, TakeFeed};
    for fed in [true, false] {
        let (checked, wrong) = unstarved(fed, || {
            let (path, values) = take(if fed { "switch-fed" } else { "switch-own" });
            let (mut engine, handle) = engine_pair(SR as f32, 2);
            let app = Arc::new(Mutex::new(AppEngine::over(handle)));
            let reading = app.lock().unwrap().reading();
            let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
            let mut playback = AudioEditorPlayback::in_medium(EngineMedium::new(Arc::clone(&app)));
            let feed = TakeFeed::start(1);
            if fed {
                let part = clausters_document::clipboard::SegmentPart {
                    path: path.clone(),
                    start: 0,
                    frames: FRAMES as u64,
                    fade_in: 0,
                    fade_out: 0,
                    fade_in_shape: Default::default(),
                    fade_out_shape: Default::default(),
                    channels: None,
                    source_channels: 1,
                    source_frames: FRAMES as u64,
                    source_rate: SR,
                };
                let list = ReadList::of(&[part], 1, SR, &mut Headers::default()).unwrap();
                feed.set(Arc::new(list));
                let buffers = AppEngine::alloc_take(1, &mut ids).unwrap();
                app.lock()
                    .unwrap()
                    .install_take(&feed, FRAMES as u64, &buffers);
                playback
                    .sync_streams(1, buffers, FRAMES as u64, SR, SR, &mut ids)
                    .unwrap();
            } else {
                let bufnum = ids.alloc(Space::Buffers, 1).unwrap() as i32;
                app.lock().unwrap().push(vec![Cmd::SetBuffer {
                    index: bufnum as usize,
                    buffer: Some(join(&path)),
                }]);
                playback
                    .sync(1, bufnum, 1, FRAMES as u64, SR, SR, &mut ids)
                    .unwrap();
            }
            let transport = playback.transport().unwrap_or(0) as usize;
            let looped = Pass::Loop {
                from: 1000,
                to: 21000,
            };
            playback.play(1, 1000, looped);
            let mut out = vec![0.0f32; BLOCK_SIZE * 2];
            let (mut on, mut next, mut sounding) = (true, 40usize, false);
            // the pass the last switch near the loop's end was turned in
            let mut near = None;
            // when the switch was last turned
            let mut last = 0;
            let (mut checked, mut wrong) = (0, Vec::new());
            for b in 0..3000usize {
                // Once a pass, the switch is turned just before the loop's end --
                // where the new reading cannot be read before the frames where the
                // two part are played, and the change is cued to the next pass.
                let at = reading.transport_position(transport) as usize;
                let pass = b / 300;
                if (20_700..21_000).contains(&at) && near != Some(pass) && b >= last + 96 {
                    near = Some(pass);
                    next = b;
                }
                if b == next {
                    last = b;
                    on = !on;
                    playback.set_pass(if on {
                        looped
                    } else {
                        Pass::Until {
                            end: Some(FRAMES as u64),
                            back: 1000,
                        }
                    });
                    // at a hand's pace: every six to twelve bursts
                    next = b + 96 + (b * 7919) % 97;
                }
                let before = reading.transport_position(transport) as usize;
                engine.process_block(&mut out);
                app.lock().unwrap().pump();
                let after = reading.transport_position(transport) as usize;
                let left: Vec<f32> = out.as_chunks::<2>().0.iter().map(|f| f[0]).collect();
                sounding |= b > 0 && left[0] != 0.0;
                // a block played straight through, past the play's ramp and
                // short of the end's
                if sounding && b > 20 && after == before + BLOCK_SIZE && after <= FRAMES - 600 {
                    checked += 1;
                    if let Some(i) =
                        (0..BLOCK_SIZE).find(|&i| (left[i] - values[before + i]).abs() > 1e-6)
                    {
                        wrong.push((b, i, left[i], values[before + i]));
                    }
                }
                if b % 16 == 15 {
                    std::thread::sleep(Duration::from_micros(1_333 * 16));
                }
            }
            let _ = std::fs::remove_file(&path);
            let starved = app.lock().unwrap().handle().meters().starved();
            ((checked, wrong), starved)
        });
        assert!(checked > 300, "fed {fed}: only {checked} blocks checked");
        assert!(
            wrong.is_empty(),
            "fed {fed}: {} of {checked} blocks wrong: {:?}",
            wrong.len(),
            &wrong[..wrong.len().min(8)]
        );
    }
}

/// **Two loop switches turned close together at the loop's end are not
/// heard**: once a pass the switch turns the loop off a little before its
/// end and on again a few blocks later -- the loop it was, or another one --
/// the second turn within a few blocks of the end or within the last one.
/// Turned back to the loop the frames kept from the first turn were read
/// through, the readers are ready on those frames the moment it is asked,
/// and it is put in place before the end; turned to another, the readers
/// keep what the loop in force plays, so a change put in place a pass
/// later is not heard either. The transport and the readers never go two
/// ways. Every block played straight through is the take's frames at its
/// position, over the fed stream and over the reader's own thread.
#[test]
fn two_loop_switches_close_together_at_its_end_are_not_heard() {
    use clausters_app_engine::take::{Headers, ReadList, TakeFeed};
    for fed in [true, false] {
        let (checked, wrong) = unstarved(fed, || {
            let (path, values) = take(if fed { "switch-fed" } else { "switch-own" });
            let (mut engine, handle) = engine_pair(SR as f32, 2);
            let app = Arc::new(Mutex::new(AppEngine::over(handle)));
            let reading = app.lock().unwrap().reading();
            let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
            let mut playback = AudioEditorPlayback::in_medium(EngineMedium::new(Arc::clone(&app)));
            let feed = TakeFeed::start(1);
            if fed {
                let part = clausters_document::clipboard::SegmentPart {
                    path: path.clone(),
                    start: 0,
                    frames: FRAMES as u64,
                    fade_in: 0,
                    fade_out: 0,
                    fade_in_shape: Default::default(),
                    fade_out_shape: Default::default(),
                    channels: None,
                    source_channels: 1,
                    source_frames: FRAMES as u64,
                    source_rate: SR,
                };
                let list = ReadList::of(&[part], 1, SR, &mut Headers::default()).unwrap();
                feed.set(Arc::new(list));
                let buffers = AppEngine::alloc_take(1, &mut ids).unwrap();
                app.lock()
                    .unwrap()
                    .install_take(&feed, FRAMES as u64, &buffers);
                playback
                    .sync_streams(1, buffers, FRAMES as u64, SR, SR, &mut ids)
                    .unwrap();
            } else {
                let bufnum = ids.alloc(Space::Buffers, 1).unwrap() as i32;
                app.lock().unwrap().push(vec![Cmd::SetBuffer {
                    index: bufnum as usize,
                    buffer: Some(join(&path)),
                }]);
                playback
                    .sync(1, bufnum, 1, FRAMES as u64, SR, SR, &mut ids)
                    .unwrap();
            }
            let transport = playback.transport().unwrap_or(0) as usize;
            let looped = Pass::Loop {
                from: 1000,
                to: 21000,
            };
            playback.play(1, 1000, looped);
            let mut out = vec![0.0f32; BLOCK_SIZE * 2];
            let (mut on, mut sounding) = (true, false);
            let (mut checked, mut wrong) = (0, Vec::new());
            let mut turned = None;
            for b in 0..3000usize {
                // Once a pass the switch is turned off a little before the
                // loop's end and on again just before it, a few blocks apart.
                let at = reading.transport_position(transport) as usize;
                let pass = b / 300;
                let turn = match turned {
                    None if b > 40 && (20_100..20_300).contains(&at) => Some((pass, 1)),
                    // the second within a few blocks of the end, or within the
                    // last one, pass by pass
                    Some((p, 1))
                        if (if p % 2 == 0 { 20_850 } else { 20_940 }..21_000).contains(&at) =>
                    {
                        Some((p, 2))
                    }
                    Some((_, 2)) if at < 20_000 => {
                        turned = None;
                        None
                    }
                    _ => None,
                };
                if let Some(t) = turn {
                    turned = Some(t);
                    on = !on;
                    playback.set_pass(if on {
                        // the loop the frames kept from the first turn read, or
                        // another one, every other pair of passes
                        match (t.0 / 2) % 2 {
                            0 => looped,
                            _ => Pass::Loop {
                                from: 2000 + 500 * (b as u64 % 3),
                                to: 21000,
                            },
                        }
                    } else {
                        Pass::Until {
                            end: Some(FRAMES as u64),
                            back: 1000,
                        }
                    });
                }
                let before = reading.transport_position(transport) as usize;
                engine.process_block(&mut out);
                app.lock().unwrap().pump();
                let after = reading.transport_position(transport) as usize;
                let left: Vec<f32> = out.as_chunks::<2>().0.iter().map(|f| f[0]).collect();
                sounding |= b > 0 && left[0] != 0.0;
                // a block played straight through, past the play's ramp and
                // short of the end's
                if sounding && b > 20 && after == before + BLOCK_SIZE && after <= FRAMES - 600 {
                    checked += 1;
                    if let Some(i) =
                        (0..BLOCK_SIZE).find(|&i| (left[i] - values[before + i]).abs() > 1e-6)
                    {
                        wrong.push((b, i, left[i], values[before + i]));
                    }
                }
                if b % 16 == 15 {
                    std::thread::sleep(Duration::from_micros(1_333 * 16));
                }
            }
            let _ = std::fs::remove_file(&path);
            let starved = app.lock().unwrap().handle().meters().starved();
            ((checked, wrong), starved)
        });
        assert!(checked > 300, "fed {fed}: only {checked} blocks checked");
        assert!(
            wrong.is_empty(),
            "fed {fed}: {} of {checked} blocks wrong: {:?}",
            wrong.len(),
            &wrong[..wrong.len().min(8)]
        );
    }
}

/// **A pass that ends on its own is taken in on the application's turn**:
/// played to the end of the take, the transport stops, and the editor hears
/// it with nothing asked of it -- its state is no longer playing, the window
/// is told, and an edit it refused while playing is not refused for that.
/// Once taken in, nothing more is said.
#[test]
fn a_pass_that_ends_on_its_own_is_taken_in() {
    use clausters_app_engine::app::App;
    use clausters_apps::editing::Editing;
    use serde_json::json;

    let (path, _) = take("ended");
    let workspace = std::env::temp_dir().join(format!("app-engine-end-{}", std::process::id()));
    std::fs::create_dir_all(&workspace).unwrap();
    let (mut engine, handle) = engine_pair(SR as f32, 2);
    let mut app = App::new(Arc::new(Mutex::new(AppEngine::over(handle))));
    let mut editing = Editing::new();
    app.hold(&mut editing);
    let opened = app.call(
        &mut editing,
        &json!({"verb": "openAudio", "key": "take", "path": path,
            "workspace": workspace.to_string_lossy(), "frames": FRAMES,
            "channels": 1, "rate": SR}),
    );
    let member = opened["member"].as_u64().expect("an audio member") as u32;
    for call in [
        json!({"verb": "sync", "window": 900}),
        json!({"verb": "window", "widget": 12}),
    ] {
        app.call(
            &mut editing,
            &json!({"verb": "member", "member": member, "call": call}),
        );
    }
    act(
        &mut app,
        &mut editing,
        &mut engine,
        member,
        json!({"verb": "open", "empty": false}),
    );
    let app_engine = Arc::clone(app.engine());
    // the last tenth of the take
    act(
        &mut app,
        &mut editing,
        &mut engine,
        member,
        json!({"verb": "play", "at": 0.45}),
    );
    let state = |app: &mut App, editing: &mut Editing| {
        app.query(editing, member, "transport")["state"].clone()
    };
    assert_eq!(state(&mut app, &mut editing)["rolling"], true);
    // the application's turn after every block, as its thread takes one: the
    // stop is heard once, at the end, and never while the pass sounds
    let mut told = Vec::new();
    let mut heard_at = None;
    for block in 0..600 {
        let left = blocks(&mut engine, &app_engine, 1);
        let ran: Vec<_> = app
            .landed(&mut editing)
            .into_iter()
            .filter(|ran| ran.member == member)
            .collect();
        if !ran.is_empty() {
            assert!(
                heard_at.is_none(),
                "heard twice, at {heard_at:?} and {block}"
            );
            heard_at = Some(block);
            told.extend(ran);
        } else if heard_at.is_none() {
            assert_eq!(
                state(&mut app, &mut editing)["rolling"],
                true,
                "block {block}: still playing, {left:?}"
            );
        }
    }
    assert!(heard_at.is_some_and(|b| b > 10), "heard at {heard_at:?}");
    assert!(
        told.iter()
            .any(|ran| ran.effects.iter().any(|e| e["kind"] == "set")),
        "the window is told: {told:?}"
    );
    let now = state(&mut app, &mut editing);
    assert_eq!(now["rolling"], false, "{now}");
    assert!(now["playing"].is_null(), "{now}");
    let actions = app.call(&mut editing, &json!({"verb": "actions", "member": member}));
    assert_ne!(
        actions["delete"]["reason"], "the file is playing: stop it to edit",
        "{actions}"
    );
    assert!(
        app.landed(&mut editing).is_empty(),
        "taken in once, and nothing more is said"
    );
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir_all(&workspace);
}

/// **A second window plays as the first does**: two files open in two
/// members of one application, over its one engine, and the second plays
/// its own take after the first played and stopped. Each member's playback
/// has a transport of its own, and the groups each one governs are its own.
#[test]
fn a_second_window_plays_on_the_engine_the_first_holds() {
    use clausters_app_engine::app::App;
    use clausters_apps::editing::Editing;
    use serde_json::json;

    let (first_path, _) = take("first-window");
    let (second_path, values) = take("second-window");
    let workspace = std::env::temp_dir().join(format!("app-engine-ws2-{}", std::process::id()));
    std::fs::create_dir_all(&workspace).unwrap();
    let (mut engine, handle) = engine_pair(SR as f32, 2);
    let mut app = App::new(Arc::new(Mutex::new(AppEngine::over(handle))));
    let mut editing = Editing::new();
    app.hold(&mut editing);
    let mut open = |app: &mut App, editing: &mut Editing, key: &str, path: &str, window: i64| {
        let opened = app.call(
            editing,
            &json!({"verb": "openAudio", "key": key, "path": path,
                "workspace": workspace.to_string_lossy(), "frames": FRAMES,
                "channels": 1, "rate": SR}),
        );
        let member = opened["member"].as_u64().expect("an audio member") as u32;
        for call in [
            json!({"verb": "sync", "window": window}),
            json!({"verb": "window", "widget": window + 12}),
        ] {
            app.call(
                editing,
                &json!({"verb": "member", "member": member, "call": call}),
            );
        }
        act(
            app,
            editing,
            &mut engine,
            member,
            json!({"verb": "open", "empty": false}),
        );
        member
    };
    let first = open(&mut app, &mut editing, "first", &first_path, 900);
    let second = open(&mut app, &mut editing, "second", &second_path, 1900);
    let app_engine = Arc::clone(app.engine());
    act(
        &mut app,
        &mut editing,
        &mut engine,
        first,
        json!({"verb": "play", "at": 0.0}),
    );
    blocks(&mut engine, &app_engine, 40);
    act(
        &mut app,
        &mut editing,
        &mut engine,
        first,
        json!({"verb": "stop"}),
    );
    blocks(&mut engine, &app_engine, 20);
    act(
        &mut app,
        &mut editing,
        &mut engine,
        second,
        json!({"verb": "play", "at": 0.0}),
    );
    let heard = blocks(&mut engine, &app_engine, 150);
    let (start, from) = rolled(&heard);
    for (i, &s) in heard.iter().enumerate().skip(from) {
        assert_eq!(
            s,
            values[i - start],
            "sample {i} of the second window's take"
        );
    }
    let output = app.query(&mut editing, second, "output");
    assert!(output["levels"][0].as_f64().unwrap() > 0.2, "{output}");
    let transports = [first, second]
        .map(|member| app.query(&mut editing, member, "transport")["transport"].clone());
    assert_ne!(
        transports[0], transports[1],
        "each window has a transport of its own"
    );
    let _ = std::fs::remove_file(&first_path);
    let _ = std::fs::remove_file(&second_path);
}
