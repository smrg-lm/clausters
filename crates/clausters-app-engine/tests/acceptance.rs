//! **The multitrack over the application's modules, measured**: sessions of
//! hour-long takes open in the time their peaks take to read, a comp of a
//! quarter-second parts plays with nothing starved and no step at a seam,
//! and memory does not grow with the takes' length.
//!
//! Hour-long takes are gigabytes, so the test writes them under the target
//! directory (on disk, never a memory-backed temporary directory) and is run
//! by hand, in release:
//!
//! ```text
//! cargo test --release -p clausters-app-engine --test acceptance -- --ignored --nocapture
//! ```

#![cfg(feature = "synth")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clausters::server::engine::{BLOCK_SIZE, engine_pair};
use clausters_app_engine::AppEngine;
use clausters_app_engine::app::App;
use clausters_apps::editing::Editing;
use clausters_document::multitrack::{Content, Fade, Multitrack, Region, TakeLane, Track, Window};
use clausters_document::session::{Session, Source};
use clausters_document::timebase::Second;
use clausters_document::{Document, Lifetime, NodeId, Opaque, SourceId, SourceRef};
use serde_json::json;

const SR: f64 = 48_000.0;
const HOUR: u64 = 3_600 * 48_000;

/// Where the takes are written: the target directory, on disk.
fn folder(name: &str) -> PathBuf {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&base).unwrap();
    base
}

/// A mono W64 take `frames` long of a smooth tone at `hz`, written once.
fn take(dir: &Path, name: &str, frames: u64, hz: f64) -> String {
    let path = dir.join(format!("{name}.w64"));
    let path = path.to_string_lossy().into_owned();
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > frames * 4) {
        return path;
    }
    let mut w = clausters_core::pcm::Writer::create(&path, 1, SR, "float").unwrap();
    let mut block = vec![0.0f32; 1 << 16];
    let mut at = 0u64;
    while at < frames {
        let n = ((frames - at) as usize).min(block.len());
        for (i, s) in block[..n].iter_mut().enumerate() {
            let t = (at + i as u64) as f64 / SR;
            *s = (0.4 * (std::f64::consts::TAU * hz * t).sin()) as f32;
        }
        w.write(&block[..n]).unwrap();
        at += n as u64;
    }
    w.finalize().unwrap();
    path
}

fn window(source: u64, start: f64, duration: f64) -> Content {
    Content::Window {
        window: Window {
            source: SourceRef {
                source: SourceId(source),
                lifetime: Lifetime::Session,
                generation: 0,
                range: None,
            },
            start,
            duration,
        },
        playrate: 1.0,
        args: Opaque::none(),
        looping: false,
    }
}

/// The resident memory of this process, in bytes.
fn resident() -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let pages: u64 = statm
        .split_whitespace()
        .nth(1)
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    pages * 4096
}

/// An application over a detached engine, its member opened over the
/// session at `path`, its peaks landed. Answers how long the open took.
fn opened(
    path: &Path,
) -> (
    clausters::server::engine::Engine,
    Arc<Mutex<AppEngine>>,
    App,
    Editing,
    u32,
    Duration,
) {
    let (engine, handle) = engine_pair(SR as f32, 2);
    let app_engine = Arc::new(Mutex::new(AppEngine::over(handle)));
    let mut app = App::new(Arc::clone(&app_engine));
    let mut editing = Editing::new();
    app.hold(&mut editing);
    let started = Instant::now();
    let opened = app.call(
        &mut editing,
        &json!({"verb": "openMultitrack", "key": "mt", "multitrack": {}, "rate": SR}),
    );
    let member = opened["member"].as_u64().unwrap() as u32;
    let ran = app.act(
        &mut editing,
        member,
        json!({"verb": "open", "session": path.to_str().unwrap()}),
        1,
        None,
    );
    assert!(matches!(ran.end, Some(Ok(_))), "{:?}", ran.end);
    let took = started.elapsed();
    (engine, app_engine, app, editing, member, took)
}

/// The peaks the open handed off, landed.
fn landed(app: &mut App, editing: &mut Editing) {
    let deadline = Instant::now() + Duration::from_secs(600);
    while app.busy() {
        let _ = app.landed(editing);
        assert!(Instant::now() < deadline, "the peaks never landed");
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = app.landed(editing);
}

#[test]
#[ignore = "writes hour-long takes; run in release with --ignored"]
fn hour_long_takes_open_on_their_peaks_and_a_comp_plays_whole() {
    let dir = folder("multitrack-acceptance");
    let names = ["a", "b", "c", "d"];
    let hz = [220.0, 275.0, 330.0, 385.0];
    let paths: Vec<String> = names
        .iter()
        .zip(hz)
        .map(|(name, hz)| take(&dir, name, HOUR, hz))
        .collect();

    // **Four tracks of an hour each**: the session, then its peaks measured
    // once, then opened again on them.
    let mut tracks = Vec::new();
    for (i, _) in paths.iter().enumerate() {
        let mut track = Track::new(NodeId(100 + i as u64), NodeId(200 + i as u64));
        track.channels = 1;
        track.take_lanes[0] = TakeLane {
            regions: vec![Region::new(
                NodeId(300 + i as u64),
                Second(0.0),
                Second(3_600.0),
                window(i as u64 + 1, 0.0, 3_600.0),
            )],
            ..TakeLane::new(NodeId(200 + i as u64))
        };
        tracks.push(track);
    }
    let mut session = Session::new(Document::empty()).with_multitrack(Multitrack {
        tracks,
        channels: 1,
        ..Multitrack::default()
    });
    for (i, path) in paths.iter().enumerate() {
        session = session.with_source(
            SourceId(i as u64 + 1),
            Source::file(path.as_str(), Lifetime::Session),
        );
    }
    let path = dir.join("hour.json");
    std::fs::write(&path, serde_json::to_string(&session).unwrap()).unwrap();
    {
        let (_e, _ae, mut app, mut editing, _m, _t) = opened(&path);
        landed(&mut app, &mut editing);
    }
    let before = resident();
    let (_engine, _app_engine, _app, _editing, _member, took) = opened(&path);
    // **What reading the peaks costs**, read alone
    let peaks: Vec<_> = std::fs::read_dir(dir.join("peaks"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    let started = Instant::now();
    let bytes: usize = peaks
        .iter()
        .map(|p| std::fs::read(p).map_or(0, |b| b.len()))
        .sum();
    let read = started.elapsed();
    let grew = resident().saturating_sub(before);
    eprintln!(
        "open: {took:?}; peaks alone: {read:?} ({bytes} bytes); resident grew {} MB",
        grew >> 20
    );
    assert!(
        took < read * 20 + Duration::from_millis(500),
        "the open read more than its peaks: {took:?} against {read:?}"
    );
    assert!(
        grew < 256 << 20,
        "an open of four hours grew the process by {} MB",
        grew >> 20
    );

    // **A comp**: 240 parts of a quarter second alternating three takes from
    // half an hour in, each crossfaded into the next.
    let mut regions = Vec::new();
    for i in 0..240u64 {
        let source = i % 3 + 1;
        let mut region = Region::new(
            NodeId(1_000 + i),
            Second(i as f64 * 0.25),
            Second(0.25 + 0.01),
            window(source, 1_800.0 + i as f64 * 0.25, 3_600.0),
        );
        region.fade_in = Some(Fade::of(Second(0.01)));
        region.fade_out = Some(Fade::of(Second(0.01)));
        regions.push(region);
    }
    let mut track = Track::new(NodeId(1), NodeId(2));
    track.channels = 1;
    track.take_lanes[0] = TakeLane {
        regions,
        ..TakeLane::new(NodeId(2))
    };
    let mut comp = Session::new(Document::empty()).with_multitrack(Multitrack {
        tracks: vec![track],
        channels: 1,
        ..Multitrack::default()
    });
    for (i, path) in paths.iter().take(3).enumerate() {
        comp = comp.with_source(
            SourceId(i as u64 + 1),
            Source::file(path.as_str(), Lifetime::Session),
        );
    }
    let path = dir.join("comp.json");
    std::fs::write(&path, serde_json::to_string(&comp).unwrap()).unwrap();
    let (mut engine, app_engine, mut app, mut editing, member, _) = opened(&path);
    landed(&mut app, &mut editing);
    app.act(&mut editing, member, json!({"verb": "play"}), 2, None);
    let seconds = 60.0;
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    let mut left = Vec::new();
    while left.len() < (seconds * SR) as usize {
        engine.process_block(&mut out);
        let _ = app.landed(&mut editing);
        left.extend(out.as_chunks::<2>().0.iter().map(|f| f[0]));
        // paced at twice the device's rate: the readers have time, as a
        // device gives them, and the test does not take a minute
        std::thread::sleep(Duration::from_micros(600));
    }
    let output = app.query(&mut editing, member, "output");
    assert_eq!(output["starved"], 0, "{output}");
    // the comp as the application reads it
    let first = left.iter().position(|&s| s != 0.0).expect("heard");
    let rolled = first / BLOCK_SIZE * BLOCK_SIZE;
    let take_files: BTreeMap<SourceId, clausters_document::parts::TakeFile> = paths
        .iter()
        .take(3)
        .enumerate()
        .map(|(i, p)| {
            (
                SourceId(i as u64 + 1),
                clausters_document::parts::TakeFile {
                    path: p.as_str().into(),
                    peaks: None,
                    channels: 1,
                    frames: HOUR,
                    rate: SR,
                },
            )
        })
        .collect();
    let shapes = take_files
        .iter()
        .map(|(id, f)| {
            (
                *id,
                clausters_document::multitrack::read::SourceShape {
                    channels: 1,
                    frames: f.frames,
                    rate: SR,
                },
            )
        })
        .collect();
    let read = clausters_document::multitrack::read::track_reads(
        &comp.multitrack,
        SR,
        &shapes,
        &Default::default(),
    )
    .remove(0);
    let list = clausters_apps::editing::held::PlacedList {
        regions: read.regions.into(),
        sources: Arc::new(
            take_files
                .into_iter()
                .map(|(id, f)| (id, clausters_apps::editing::held::PlacedSource::File(f)))
                .collect(),
        ),
        frames: read.frames,
    };
    let placed = clausters_app_engine::take::PlacedRead::over(
        &list,
        1,
        &mut clausters_app_engine::take::Headers::default(),
        &Arc::default(),
    )
    .unwrap();
    let n = left.len() - rolled - 256;
    let expected = placed.read(256, n.min(55 * SR as usize), 0);
    let worst = expected
        .iter()
        .enumerate()
        .map(|(i, want)| (left[rolled + 256 + i] - want).abs())
        .fold(0.0f32, f32::max);
    eprintln!("comp: {} parts, worst difference {worst}", 240);
    assert!(worst < 1e-5, "a seam is not what the comp reads: {worst}");
    let _ = app_engine;
}
