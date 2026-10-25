//! **A copy pasted over and over, to more than a hundred hours**: the audio
//! editor's take is a tree of shared parts, so each paste answers in the
//! same time however long the take and however many pastes came before it,
//! the window is told the new take in the same few bytes and draws it from
//! the nodes its columns hold, and memory follows the tree's nodes rather
//! than its parts.
//!
//! The pattern is the one that hung the editor: ten seconds of a take
//! copied and pasted five times at its end make a minute; that minute
//! copied whole and pasted nine times makes ten; then the whole six times,
//! six times, four times and three times -- 190 hours of 58,800 parts. Each
//! paste is timed on the editor, and what it says to the window (the take
//! by reference) is handed to the host's answerer as the application's
//! thread hands it, and the whole take drawn at the column width a window
//! of 760 columns draws it, from the file's peaks. Then every paste is
//! undone and redone, each timed as the pastes were.
//!
//! A measurement, `--ignored`, and in release:
//!
//! ```sh
//! cargo test --release --test shared_blocks -- --ignored --nocapture
//! ```

use std::time::Instant;

use clausters_apps::audio::editor::{AudioEditor, call_json, new_json};
use clausters_apps::audio::shared;
use clausters_apps::turn::Event;
use clausters_core::osc::{OscMessage, OscType};
use clausters_core::peaks::MultiPyramid;
use clausters_gui::host::disk::answer::{Answer, Answerer};
use clausters_gui::host::disk::native::Disk;
use clausters_gui::host::disk::{List, receive};
use serde_json::{Value, json};

const SR: usize = 48_000;
const CHANNELS: usize = 2;
const SECONDS: usize = 20;
const FRAMES: usize = SR * SECONDS;
const PICTURE: i64 = 10;
const COLUMNS: usize = 760;

/// Twenty seconds of stereo as a float W64, and its peaks where the editor
/// keeps the peaks of a file it opened.
fn write_take(path: &str, peaks: &str) {
    let mut w = clausters_core::pcm::Writer::create(path, CHANNELS, SR as f64, "float").unwrap();
    let mut s = 1u32;
    let mut block = vec![0.0f32; FRAMES * CHANNELS];
    for x in block.iter_mut() {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        *x = (s as f32 / u32::MAX as f32) * 2.0 - 1.0;
    }
    w.write(&block).unwrap();
    w.finalize().unwrap();
    let mut stats = Vec::new();
    for bucket in block.chunks(256 * CHANNELS) {
        let n = bucket.len() / CHANNELS;
        for ch in 0..CHANNELS {
            let v: Vec<f32> = (0..n).map(|f| bucket[f * CHANNELS + ch]).collect();
            let lo = v.iter().copied().fold(f32::INFINITY, f32::min);
            let hi = v.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let ms = v.iter().map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>() / n as f64;
            stats.extend([lo, hi, ms as f32]);
        }
    }
    let mut pyramid = MultiPyramid::empty(FRAMES, CHANNELS, 256);
    assert!(pyramid.write_buckets(0, 256, &stats));
    pyramid.write_cache(peaks).unwrap();
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

fn event(editor: &mut AudioEditor, args: Value, version: i64) -> (Value, usize) {
    let args: Vec<Value> = args.as_array().unwrap().clone();
    let out = serde_json::to_value(editor.event(
        &Event {
            addr: "/gui_event".into(),
            args,
        },
        version,
    ))
    .unwrap();
    let bytes = out.to_string().len();
    (out, bytes)
}

/// The coarsest bucket that gives each of `COLUMNS` at least one, as the
/// fetch machine picks it.
fn level(frames: usize) -> usize {
    let mut bucket = 256;
    while bucket * 2 <= frames / COLUMNS {
        bucket *= 2;
    }
    bucket
}

/// **The whole take drawn at `bucket`**, walked as the fetch machine walks
/// it, every read the answerer asks for carried out on the disk. Answers
/// how many buckets came back.
fn picture(answerer: &mut Answerer, disk: &mut Disk, bucket: usize) -> usize {
    let mut next = 0i64;
    let mut buckets = 0;
    loop {
        let msg = OscMessage {
            addr: "/buffer_peaks".into(),
            args: vec![
                OscType::Int(-2),
                OscType::Int(bucket as i32),
                OscType::Long(next),
                OscType::Int(-1),
            ],
        };
        let reply = loop {
            match answerer.answer(&msg) {
                Answer::Reply(reply) => break reply,
                Answer::None => panic!("no answer to {msg:?}"),
                Answer::Need(needs) => {
                    for need in needs {
                        let got = disk.read(&need);
                        answerer.deliver(&need, got);
                    }
                }
            }
        };
        let OscType::Blob(blob) = &reply.args[3] else {
            panic!("{reply:?}");
        };
        let n = blob.len() / 4 / (CHANNELS * 3);
        if n == 0 {
            return buckets;
        }
        buckets += n;
        next += (n * bucket) as i64;
    }
}

/// One paste, as measured.
struct Paste {
    ms: f64,
    told: usize,
    draw_ms: f64,
    parts_read: usize,
}

#[test]
#[ignore = "a measurement: run in release with --ignored --nocapture"]
fn a_copy_pasted_to_a_hundred_hours_costs_what_its_first_paste_did() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("shared-blocks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("take.w64").to_string_lossy().into_owned();
    let work = dir.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let mut editor = new_json(
        &json!({"path": path, "workspace": work.to_string_lossy(), "frames": FRAMES,
                "channels": CHANNELS, "rate": SR})
        .to_string(),
    )
    .unwrap();
    // the application's own engine reads the take: nothing is stitched
    call_json(
        &mut editor,
        &json!({"verb": "sync", "window": 900, "rate": SR, "held": true}).to_string(),
    );
    call_json(
        &mut editor,
        &json!({"verb": "window", "widget": PICTURE}).to_string(),
    );
    let wire: Value =
        serde_json::from_str(&call_json(&mut editor, r#"{"verb":"segments"}"#)).unwrap();
    let peaks = wire["parts"][0]["peaks"].as_str().unwrap().to_string();
    write_take(&path, &peaks);

    let mut answerer = Answerer::default();
    let mut disk = Disk::default();
    let mut version = 1;
    let mut seq = 1;
    let mut pastes: Vec<Paste> = Vec::new();
    let mut legs: Vec<Value> = Vec::new();
    let before = resident();
    let mut copied = (10 * SR) as u64;
    for times in [5, 9, 6, 6, 4, 3] {
        seq += 1;
        let (out, _) = event(
            &mut editor,
            json!([PICTURE, seq, version, "copy", 0, copied]),
            version,
        );
        let doc = out["clipboard"].as_str().expect("a copy").to_string();
        assert!(
            doc.len() < 512,
            "a copy is a reference: {} bytes",
            doc.len()
        );
        for _ in 0..times {
            seq += 1;
            let at = editor.length();
            let started = Instant::now();
            let (out, told) = event(
                &mut editor,
                json!([PICTURE, seq, version, "paste", at, "segments", doc]),
                version,
            );
            let ms = started.elapsed().as_secs_f64() * 1e3;
            version = out["version"].as_i64().unwrap_or(version + 1);
            legs.push(out["record"]["legs"][0].clone());
            // **What the window is told**, as the application's thread
            // hands it over: the take by reference, and its number.
            let started = Instant::now();
            receive(shared::said());
            let segments = out["answer"]["corrections"]
                .as_array()
                .unwrap()
                .iter()
                .find_map(|c| c["props"].get("segments"))
                .expect("the picture is told the take");
            let list = List::parse(segments).expect("a list by reference");
            assert_eq!(list.frames as u64, editor.length());
            answerer.define(-2, list);
            let read = answerer.parts_read;
            let buckets = picture(&mut answerer, &mut disk, level(editor.length() as usize));
            assert!(buckets > 0);
            pastes.push(Paste {
                ms,
                told,
                draw_ms: started.elapsed().as_secs_f64() * 1e3,
                parts_read: answerer.parts_read - read,
            });
        }
        copied = editor.length();
    }
    // **Every paste undone and redone**: each a take the history names by
    // number, put back
    let length = editor.length();
    let mut steps: Vec<f64> = Vec::new();
    for leg in legs.iter().rev() {
        let started = Instant::now();
        assert!(editor.apply(&leg["backward"]), "an undo");
        steps.push(started.elapsed().as_secs_f64() * 1e3);
    }
    assert_eq!(editor.length() as usize, FRAMES, "all undone");
    for leg in &legs {
        let started = Instant::now();
        assert!(editor.apply(&leg["forward"]["edit"]), "a redo");
        steps.push(started.elapsed().as_secs_f64() * 1e3);
    }
    assert_eq!(editor.length(), length, "all redone");
    let slowest_step = steps.iter().copied().fold(0.0, f64::max);
    let grown = resident().saturating_sub(before);
    let hours = editor.length() as f64 / SR as f64 / 3_600.0;
    let parts = editor.list().len();
    for (n, p) in pastes.iter().enumerate() {
        println!(
            "paste {:2}: {:7.3} ms, told {:5} bytes, drawn in {:7.2} ms reading {:5} parts",
            n + 1,
            p.ms,
            p.told,
            p.draw_ms,
            p.parts_read
        );
    }
    println!(
        "{hours:.1} h of {parts} parts after {} pastes, undone and redone \
         ({slowest_step:.3} ms the slowest step); memory grew {} KB",
        pastes.len(),
        grown / 1024
    );
    assert!(hours > 100.0, "{hours} hours");
    assert!(parts > 40_000, "{parts} parts");
    let first = &pastes[..5];
    let slowest_first = first.iter().map(|p| p.ms).fold(0.0, f64::max);
    let told_first = first.iter().map(|p| p.told).max().unwrap();
    for (n, p) in pastes.iter().enumerate() {
        assert!(
            p.ms <= 10.0 * slowest_first + 1.0,
            "paste {}: {:.3} ms against {slowest_first:.3} ms at first",
            n + 1,
            p.ms
        );
        assert!(
            p.told <= 2 * told_first,
            "paste {}: the window was told {} bytes against {told_first} at first",
            n + 1,
            p.told
        );
        // a picture is the nodes its columns hold: the parts it cuts at the
        // columns' edges, and those a paste made new
        assert!(
            p.parts_read <= 8 * COLUMNS * CHANNELS,
            "paste {}: the picture read {} parts",
            n + 1,
            p.parts_read
        );
    }
    assert!(
        slowest_step <= 10.0 * slowest_first + 1.0,
        "an undo or a redo took {slowest_step:.3} ms against {slowest_first:.3} ms a paste"
    );
    assert!(grown < 64 << 20, "memory grew {grown} bytes");
    let _ = std::fs::remove_dir_all(&dir);
}
