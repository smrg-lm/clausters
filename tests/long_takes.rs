//! **The audio editor over takes of hours, measured.** A take is a list of
//! parts over files, so an edit costs the list and never the samples: ten
//! pastes of an hour into a take of two hours, and into a W64 past 4 GB, each
//! answer in the time a paste into a take of two seconds does, render
//! nothing, read nothing, and leave the process's memory where it was --
//! across the ten, and across their undo and redo.
//!
//! The long takes are **sparse files**: a real W64 header over a body the
//! disk never holds, so the test costs no gigabytes and the editor, which
//! reads only headers, cannot tell. It is a measurement, `--ignored`, and in
//! release:
//!
//! ```sh
//! cargo test --release --test long_takes -- --ignored --nocapture
//! ```
#![cfg(not(target_arch = "wasm32"))]

use std::time::Instant;

use clausters::clausters_core::pcm::{Container, Encoding, header};
use clausters_apps::audio::editor::{AudioEditor, call_json, new_json};
use clausters_apps::turn::Event;
use clausters_editing::apply::{Step, steps_from_json};
use serde_json::{Value, json};

const SR: f64 = 48_000.0;
const HOUR: u64 = 3_600 * 48_000;
const WINDOW: i64 = 900;
const PICTURE: i64 = 10;

/// A directory of this test's own on the disk cargo gives integration tests.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("long-takes-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A stereo float W64 of `frames` frames whose body is never written: its
/// header, and the length the header says, as a hole.
fn sparse_take(dir: &std::path::Path, name: &str, frames: u64) -> String {
    let path = dir.join(name);
    let bytes = frames * 2 * 4;
    let head = header(Container::W64, 2, SR, Encoding::F32, bytes).unwrap();
    std::fs::write(&path, &head).unwrap();
    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_len(head.len() as u64 + bytes).unwrap();
    path.to_string_lossy().into_owned()
}

/// The resident memory of this process, in bytes.
fn resident() -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let pages: u64 = statm
        .split_whitespace()
        .nth(1)
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    pages * 4096
}

fn event(editor: &mut AudioEditor, args: Value, version: i64) -> Value {
    let args: Vec<Value> = args.as_array().unwrap().clone();
    serde_json::to_value(editor.event(
        &Event {
            addr: "/gui_event".into(),
            args,
        },
        version,
    ))
    .unwrap()
}

/// What a turn's steps come to: renders, and the messages sent.
fn kinds(steps: &Value) -> (usize, Vec<String>) {
    let mut renders = 0;
    let mut sent = Vec::new();
    for step in steps_from_json(steps) {
        match step {
            Step::Render(_) => renders += 1,
            Step::Send(m) => sent.push(m.addr),
            _ => {}
        }
    }
    (renders, sent)
}

/// One take's ten pastes of `span` frames, and their undo and redo, timed.
/// Answers the slowest turn, in milliseconds.
fn ten_pastes(path: &str, frames: u64, span: u64, what: &str) -> f64 {
    let dir = scratch(&format!("{what}-work"));
    let mut editor = new_json(
        &json!({"path": path, "workspace": dir.to_string_lossy(), "frames": frames,
                "channels": 2, "rate": SR, "display": 9})
        .to_string(),
    )
    .unwrap();
    call_json(
        &mut editor,
        &json!({"verb": "sync", "window": WINDOW, "rate": SR}).to_string(),
    );
    call_json(
        &mut editor,
        &json!({"verb": "window", "widget": PICTURE}).to_string(),
    );
    let mut version = 1;
    let copied = event(
        &mut editor,
        json!([PICTURE, 1, version, "copy", 0, span]),
        version,
    );
    let doc = copied["clipboard"].as_str().expect("a copy").to_string();
    let before = resident();
    let mut slowest: f64 = 0.0;
    let mut times = Vec::new();
    let mut legs = Vec::new();
    let mut stitches = Vec::new();
    let mut timed = |ms: f64, times: &mut Vec<f64>| {
        times.push(ms);
        slowest = slowest.max(ms);
    };
    for n in 0..10u64 {
        let at = (n * span / 2).min(frames);
        let started = Instant::now();
        let out = event(
            &mut editor,
            json!([PICTURE, 2 + n, version, "paste", at, "segments", doc]),
            version,
        );
        timed(started.elapsed().as_secs_f64() * 1e3, &mut times);
        version = out["version"].as_i64().unwrap_or(version + 1);
        let (renders, sent) = kinds(&out["steps"]);
        assert_eq!(renders, 0, "{what}: a paste of a copy renders nothing");
        assert_eq!(sent, ["/buffer_stitch"], "{what}: and sends one stitch");
        legs.push(out["record"]["legs"][0].clone());
        stitches.push(out["steps"].clone());
    }
    let length = |editor: &mut AudioEditor| {
        serde_json::from_str::<Value>(&call_json(editor, r#"{"verb":"parts"}"#)).unwrap()["frames"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(
        length(&mut editor),
        frames + 10 * span,
        "{what}: ten spans in"
    );
    // undo the ten, and redo them: each a list, stitched
    for leg in legs.iter().rev() {
        let started = Instant::now();
        let steps = editor.apply(&leg["backward"]).expect("an undo");
        timed(started.elapsed().as_secs_f64() * 1e3, &mut times);
        assert_eq!(kinds(&steps).0, 0, "{what}: an undo renders nothing");
    }
    assert_eq!(length(&mut editor), frames, "{what}: undone");
    for leg in &legs {
        let started = Instant::now();
        let steps = editor.apply(&leg["forward"]["edit"]).expect("a redo");
        timed(started.elapsed().as_secs_f64() * 1e3, &mut times);
        assert_eq!(kinds(&steps).0, 0, "{what}: a redo renders nothing");
    }
    assert_eq!(length(&mut editor), frames + 10 * span, "{what}: redone");
    let grown = resident().saturating_sub(before);
    println!(
        "{what}: {:.1} h, ten pastes of {:.2} h and their undo and redo: \
         {:.3} ms slowest, {:.3} ms median; memory grew {} KB",
        frames as f64 / SR / 3_600.0,
        span as f64 / SR / 3_600.0,
        slowest,
        median(&mut times),
        grown / 1024
    );
    assert!(
        grown < 16 << 20,
        "{what}: memory grew {grown} bytes over ten pastes"
    );
    #[cfg(feature = "synth")]
    server_holds_the_lists(&stitches, what);
    let _ = std::fs::remove_dir_all(&dir);
    slowest
}

/// **The server holds the ten lists and not their samples**: each paste's
/// stitch, carried out on an offline server of this process, leaves its
/// memory where it was.
#[cfg(feature = "synth")]
fn server_holds_the_lists(stitches: &[Value], what: &str) {
    use clausters::server::nrtsession::{NrtSession, SessionConfig};
    let mut server = NrtSession::open(&SessionConfig {
        sample_rate: SR,
        channels: 2,
        ..Default::default()
    })
    .expect("open");
    server.settle_for(4);
    let before = resident();
    let started = Instant::now();
    for steps in stitches {
        for step in steps_from_json(steps) {
            if let Step::Send(m) = step {
                assert!(server.send_msg(&m.addr, m.args).expect("encode"));
            }
        }
        server.settle_for(8);
    }
    // every stitch answered, none refused
    let (mut done, mut failed) = (0, Vec::new());
    let mut bytes = vec![0u8; 1 << 16];
    for _ in 0..200 {
        while let Some(len) = server.poll_into(&mut bytes) {
            if let Ok(rosc::OscPacket::Message(reply)) =
                clausters::osc::decode_packet(&bytes[..len])
            {
                match reply.addr.as_str() {
                    "/done" => done += 1,
                    "/fail" => failed.push(format!("{:?}", reply.args)),
                    _ => {}
                }
            }
        }
        if done == stitches.len() || !failed.is_empty() {
            break;
        }
        server.settle_for(4);
    }
    assert!(failed.is_empty(), "{what}: {failed:?}");
    assert_eq!(done, stitches.len(), "{what}: every stitch answered");
    let ms = started.elapsed().as_secs_f64() * 1e3;
    let grown = resident().saturating_sub(before);
    println!(
        "{what}: the server stitched the ten lists in {ms:.1} ms; its memory grew {} KB",
        grown / 1024
    );
    assert!(
        grown < 16 << 20,
        "{what}: the server's memory grew {grown} bytes over ten stitches"
    );
}

fn median(times: &mut [f64]) -> f64 {
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    times[times.len() / 2]
}

/// **Ten pastes of an hour cost what a paste into two seconds does**: over a
/// take of two seconds, one of two hours and a W64 past 4 GB, the slowest
/// turn of each is within a few times the short take's, and none renders,
/// reads or grows the process's memory.
#[test]
#[ignore = "a measurement: run in release with --ignored --nocapture"]
fn ten_pastes_of_an_hour_cost_what_one_into_two_seconds_does() {
    let dir = scratch("takes");
    let short = sparse_take(&dir, "short.w64", 2 * 48_000);
    let two_hours = sparse_take(&dir, "two-hours.w64", 2 * HOUR);
    // three and a half hours of stereo float: 4.8 GB of body
    let past_4gb = sparse_take(&dir, "past-4gb.w64", 7 * HOUR / 2);
    assert!(std::fs::metadata(&past_4gb).unwrap().len() > 4 << 30);
    let base = ten_pastes(&short, 2 * 48_000, 48_000, "two seconds");
    let long = ten_pastes(&two_hours, 2 * HOUR, HOUR, "two hours");
    let huge = ten_pastes(&past_4gb, 7 * HOUR / 2, HOUR, "past 4 GB");
    for (what, ms) in [("two hours", long), ("past 4 GB", huge)] {
        assert!(
            ms <= 4.0 * base + 1.0,
            "{what}: the slowest paste took {ms:.3} ms against {base:.3} ms into two seconds"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
